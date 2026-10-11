// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Cookie mirror (owned by the cookies worker).
//!
//! Why a copy instead of asking Qt: `QWebEngineCookieStore` can only enumerate
//! asynchronously (`loadAllCookies()`) and **never signals completion** — it
//! just emits one `cookieAdded` per cookie. The trait's `cookies()` is
//! synchronous and our scheme handler runs synchronously on the GUI thread, so
//! blocking on a nested event loop is not an option (re-entrancy + deadlock).
//! Instead we subscribe to `cookieAdded` / `cookieRemoved` (Qt does the
//! decryption and normalisation for us) and answer reads from this mirror.
//!
//! Semantics for callers: the mirror is an **observed snapshot**.
//! * An empty mirror is a legitimate "no cookies observed" answer, not an error.
//! * Cookies whose only change was server-side expiry, or cookies that were in
//!   the store before `qt_cookies_watch` ran (i.e. before the webview was
//!   created), appear only after the initial `loadAllCookies()` / navigation.
//! * `set_cookie` / `delete_cookie` update the mirror optimistically so a write
//!   is visible to the very next read. The authoritative `cookieAdded` /
//!   `cookieRemoved` event for the same name/domain/path then overwrites or
//!   removes that entry (idempotent upsert/remove, so event order does not
//!   matter). A write Chromium rejects outright (e.g. `SameSite=None` without
//!   `Secure`, or a domain that cannot be set from the synthesised origin) fires
//!   no event, so its provisional entry stays until something replaces it.
//! * Events for a webview that was never registered (or whose view was already
//!   destroyed) are ignored.
//!
//! What the Qt layer can and cannot carry (verified against Qt 6.11.2's
//! `QNetworkCookie` / cookie-store code):
//! * The domain of an event is Chromium's canonical form: a bare host for a
//!   host-only cookie, a dot-prefixed domain (".example.com") for a domain
//!   cookie, and the lowercased host for IP literals. `canonical_domain()`
//!   below reproduces that rule for optimistic writes.
//! * `same_site` is lossy in **both** directions through Qt: the
//!   `CanonicalCookie` -> `QNetworkCookie` conversion used for the signals never
//!   fills `sameSitePolicy()` (so events always report `Default`), and a write
//!   that carries a policy is overwritten by that same Default value as soon as
//!   the event arrives. The mirror therefore reports what Qt reports.
//! * CHIPS (`Cookie::partitioned()`) has no equivalent in the Qt API: the flag
//!   is dropped and the cookie is stored unpartitioned.
//! * `clear_all_browsing_data` is broader than cookies (HTTP cache, and by
//!   contract tauri's "all browsing data"); `clear_all_browsing_data()` here
//!   clears cookies + HTTP cache and documents the rest.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use cookie::SameSite;
use parking_lot::Mutex;
use tauri_runtime::{Cookie, Error, Result};

use crate::bridge::cookies::ffi;

/// One cookie as the Qt store reported it (or as a write asked for it).
#[derive(Debug, Clone)]
struct Snapshot {
    name: String,
    value: String,
    /// Chromium's canonical domain (see `canonical_domain`).
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    same_site: Option<SameSite>,
    /// Unix epoch milliseconds; `None` for a session cookie.
    expires_ms: Option<i64>,
}

/// Chromium identifies a cookie by name + domain + path; the mirror does too.
type Key = (String, String, String);

/// The per-process mirror.
#[derive(Default)]
struct Mirror {
    /// webview id -> observed cookies.
    cookies: HashMap<u64, HashMap<Key, Snapshot>>,
    /// webview id -> native `QWebEngineView*`, for the FFI calls.
    view_ptrs: HashMap<u64, u64>,
    /// native pointer -> webview id; the only handle Qt's signals carry.
    ptr_to_id: HashMap<u64, u64>,
}

static MIRROR: LazyLock<Mutex<Mirror>> = LazyLock::new(|| Mutex::new(Mirror::default()));

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_millis() as i64)
        .unwrap_or(0)
}

/// The domain form Chromium (and therefore the Qt events) uses for a cookie
/// written with `requested`:
///
/// * no domain -> host-only for the webview's current host (`fallback_host`);
/// * a domain whose host has a registrable domain -> `.domain` (Chromium
///   prepends the dot, matching RFC 6265's domain cookie);
/// * a bare single-label host (`localhost`) or an IP literal -> kept as-is,
///   which Chromium stores as a host-only cookie.
///
/// `requested` is regularised first: the leading dot is dropped, the string is
/// lowercased.
fn canonical_domain(requested: Option<&str>, fallback_host: &str) -> String {
    let requested = requested.map(str::trim).filter(|domain| !domain.is_empty());
    let Some(requested) = requested else {
        return fallback_host.to_ascii_lowercase();
    };
    let domain = requested.trim_start_matches('.').to_ascii_lowercase();
    let is_ip_literal = domain.parse::<IpAddr>().is_ok();
    if is_ip_literal || !domain.contains('.') {
        domain
    } else {
        format!(".{domain}")
    }
}

/// Chromium's domain match: a host-only cookie matches the exact host, a
/// dot-prefixed domain cookie matches that domain and every subdomain.
fn domain_matches(cookie_domain: &str, host: &str) -> bool {
    match cookie_domain.strip_prefix('.') {
        Some(suffix) => host == suffix || host.ends_with(cookie_domain),
        None => host == cookie_domain,
    }
}

/// The cookie's path must be a prefix of the request path (empty -> "/").
fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    let cookie_path = if cookie_path.is_empty() {
        "/"
    } else {
        cookie_path
    };
    request_path.starts_with(cookie_path)
}

/// The `same_site` code the C++ bridge expects (0 = default/absent).
fn same_site_code(same_site: Option<SameSite>) -> i32 {
    match same_site {
        Some(SameSite::None) => 1,
        Some(SameSite::Lax) => 2,
        Some(SameSite::Strict) => 3,
        None => 0,
    }
}

/// Inverse of `same_site_code` for the Qt side of the bridge. An unknown code
/// (a newer Qt) is treated as "attribute absent" instead of guessing.
fn same_site_from_code(code: i32) -> Option<SameSite> {
    match code {
        1 => Some(SameSite::None),
        2 => Some(SameSite::Lax),
        3 => Some(SameSite::Strict),
        _ => None,
    }
}

/// Expiry of a cookie about to be written, as Unix epoch milliseconds
/// (`-1` = session cookie). `max_age` wins when `expires` is absent, matching
/// HTTP semantics; `Max-Age: 0` therefore means "expire immediately".
fn expires_ms_of(cookie: &Cookie<'_>) -> i64 {
    if let Some(expires) = cookie.expires_datetime() {
        return expires.unix_timestamp() * 1_000 + i64::from(expires.millisecond());
    }
    match cookie.max_age() {
        Some(max_age) => {
            let delta = max_age.whole_milliseconds();
            let delta = delta.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
            now_ms().saturating_add(delta)
        }
        None => -1,
    }
}

/// Snapshot -> the trait's public cookie type.
fn to_public_cookie(snapshot: &Snapshot) -> Cookie<'static> {
    let mut builder = Cookie::build((snapshot.name.clone(), snapshot.value.clone()))
        .domain(snapshot.domain.clone())
        .path(snapshot.path.clone());
    if snapshot.secure {
        builder = builder.secure(true);
    }
    if snapshot.http_only {
        builder = builder.http_only(true);
    }
    if let Some(same_site) = snapshot.same_site {
        builder = builder.same_site(same_site);
    }
    if let Some(expires_ms) = snapshot.expires_ms {
        // `time` 0.3.55 has no `from_unix_timestamp_millis`; nanos cover it.
        let nanos = i128::from(expires_ms) * 1_000_000;
        if let Ok(expires) = cookie::time::OffsetDateTime::from_unix_timestamp_nanos(nanos) {
            builder = builder.expires(expires);
        }
    }
    builder.build()
}

/// The webview's current URL host, via the existing webview bridge ("" when the
/// webview has no page URL yet, e.g. before its first navigation).
fn current_host(webview_id: u64) -> String {
    let url = crate::bridge::webview::ffi::qt_webview_url(webview_id);
    url::Url::parse(&url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default()
}

/// Runs `f` on the mirror of `webview_id`, or fails when the webview was never
/// registered (unknown handle -> `WindowNotFound`, like the other ops).
fn with_webview<T>(webview_id: u64, f: impl FnOnce(&mut HashMap<Key, Snapshot>) -> T) -> Result<T> {
    let mut mirror = MIRROR.lock();
    match mirror.cookies.get_mut(&webview_id) {
        Some(cookies) => Ok(f(cookies)),
        None => Err(Error::WindowNotFound),
    }
}

/// The native view pointer of a registered webview.
fn view_ptr_of(webview_id: u64) -> Result<u64> {
    MIRROR
        .lock()
        .view_ptrs
        .get(&webview_id)
        .copied()
        .ok_or(Error::WindowNotFound)
}

/// Starts mirroring the cookie store of the given webview.
///
/// Never returns an error because it runs inside webview creation: the view is
/// recorded either way, so later reads answer "no cookies observed" instead of
/// failing window creation. The C++ side logs nothing; a failure to watch
/// (unknown pointer, no cookie store) simply leaves the mirror empty.
pub(crate) fn register_webview(webview_id: u64, webview_ptr: u64) {
    {
        let mut mirror = MIRROR.lock();
        mirror.view_ptrs.insert(webview_id, webview_ptr);
        mirror.ptr_to_id.insert(webview_ptr, webview_id);
        mirror.cookies.insert(webview_id, HashMap::new());
    }
    ffi::qt_cookies_watch(webview_ptr);
        crate::qtrace!(
        "[tauri-runtime-qt] cookie mirror watching webview {webview_id} (view {webview_ptr:#x})"
    );
}

/// A cookie entered the store (including the initial load). Called from the Qt
/// signal; the entry is upserted under Chromium's canonical key, so a repeated
/// `loadAllCookies()` or an event after an optimistic write is idempotent.
pub(crate) fn on_cookie_added(
    webview_ptr: u64,
    name: &str,
    value: &str,
    domain: &str,
    path: &str,
    secure: bool,
    http_only: bool,
    same_site: i32,
    expires_ms: i64,
) {
    let mut mirror = MIRROR.lock();
    let Some(&webview_id) = mirror.ptr_to_id.get(&webview_ptr) else {
        return; // view never registered, or already destroyed
    };
    let snapshot = Snapshot {
        name: name.to_string(),
        value: value.to_string(),
        domain: domain.to_ascii_lowercase(),
        path: path.to_string(),
        secure,
        http_only,
        same_site: same_site_from_code(same_site),
        expires_ms: (expires_ms >= 0).then_some(expires_ms),
    };
    let Some(cookies) = mirror.cookies.get_mut(&webview_id) else {
        return;
    };
    cookies.insert(
        (
            snapshot.name.clone(),
            snapshot.domain.clone(),
            snapshot.path.clone(),
        ),
        snapshot,
    );
}

/// A cookie left the store; removes the canonical key.
pub(crate) fn on_cookie_removed(webview_ptr: u64, name: &str, domain: &str, path: &str) {
    let mut mirror = MIRROR.lock();
    let Some(&webview_id) = mirror.ptr_to_id.get(&webview_ptr) else {
        return;
    };
    if let Some(cookies) = mirror.cookies.get_mut(&webview_id) {
        cookies.remove(&(
            name.to_string(),
            domain.to_ascii_lowercase(),
            path.to_string(),
        ));
    }
}

// ---- trait surface ----

/// Every cookie the mirror has observed for this webview, sorted by
/// (domain, path, name) for deterministic output. Expired entries can appear
/// here (an expiry-only change emits no event); `cookies_for_url` filters them.
pub(crate) fn cookies(webview_id: u64) -> Result<Vec<Cookie<'static>>> {
    with_webview(webview_id, |cookies| {
        let mut snapshots: Vec<&Snapshot> = cookies.values().collect();
        snapshots.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
        snapshots.into_iter().map(to_public_cookie).collect()
    })
}

/// The cookies a request to `url` would carry, matching Chromium's rules:
///
/// * a host-only cookie (domain without a leading dot) matches the exact host;
/// * a domain cookie (leading dot) matches that host and every subdomain;
/// * the cookie's path must be a prefix of the request path (empty -> "/");
/// * `Secure` cookies are returned for `https`/`wss` requests only;
/// * expired cookies are skipped.
///
/// Deliberate difference from Chromium: the scheme gate (Chromium only answers
/// for `http`/`https`/`ws`/`wss`) is not applied, so callers can ask about the
/// runtime's own `tauri://` pages; host and path still decide the match.
pub(crate) fn cookies_for_url(webview_id: u64, url: &url::Url) -> Result<Vec<Cookie<'static>>> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let request_path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let secure_request = matches!(url.scheme(), "https" | "wss");
    let now = now_ms();
    with_webview(webview_id, |cookies| {
        let mut snapshots: Vec<&Snapshot> = cookies
            .values()
            .filter(|cookie| {
                domain_matches(cookie.domain.as_str(), &host)
                    && path_matches(cookie.path.as_str(), request_path)
                    && (!cookie.secure || secure_request)
                    && cookie.expires_ms.is_none_or(|expires_ms| expires_ms > now)
            })
            .collect();
        snapshots.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
        snapshots.into_iter().map(to_public_cookie).collect()
    })
}

/// Writes a cookie through Qt and records it optimistically.
///
/// A cookie without a domain becomes host-only for the webview's current page
/// host; before the first navigation there is no host to attach it to, which is
/// an error rather than a silently dropped write.
pub(crate) fn set_cookie(webview_id: u64, cookie: Cookie<'_>) -> Result<()> {
    let view_ptr = view_ptr_of(webview_id)?;
    let host = current_host(webview_id);
    let domain = canonical_domain(cookie.domain(), &host);
    if domain.is_empty() {
        return Err(Error::CreateWebview(
            "tauri-runtime-qt: cannot set a cookie without a domain before the webview has a page URL"
                .into(),
        ));
    }
    let name = cookie.name().to_string();
    let value = cookie.value().to_string();
    let path = match cookie.path() {
        Some(path) if !path.is_empty() => path.to_string(),
        _ => "/".to_string(),
    };
    let secure = cookie.secure().unwrap_or(false);
    let http_only = cookie.http_only().unwrap_or(false);
    let same_site = cookie.same_site();
    let expires_ms = expires_ms_of(&cookie);

        crate::qtrace!(
        "[tauri-runtime-qt] set_cookie -> view {view_ptr:#x} name={name:?} value={value:?} domain={domain:?} path={path:?} secure={secure} http_only={http_only} same_site={} expires_ms={expires_ms}",
        same_site_code(same_site)
    );
    ffi::qt_cookies_set(
        view_ptr,
        &name,
        &value,
        &domain,
        &path,
        secure,
        http_only,
        same_site_code(same_site),
        expires_ms,
    );

    let snapshot = Snapshot {
        name: name.clone(),
        value,
        domain: domain.clone(),
        path: path.clone(),
        secure,
        http_only,
        same_site,
        expires_ms: (expires_ms >= 0).then_some(expires_ms),
    };
    let mut mirror = MIRROR.lock();
    if let Some(cookies) = mirror.cookies.get_mut(&webview_id) {
        cookies.insert((name, domain, path), snapshot);
    }
    Ok(())
}

/// Deletes cookies through Qt and removes them from the mirror optimistically.
///
/// Qt builds Chromium's deletion filter from the cookie *name* plus a URL
/// derived from the domain, so every path variant of that name for the host is
/// deleted; the optimistic removal mirrors that (name + domain match + path
/// prefix) rather than only the exact key.
pub(crate) fn delete_cookie(webview_id: u64, cookie: Cookie<'_>) -> Result<()> {
    let view_ptr = view_ptr_of(webview_id)?;
    let host = current_host(webview_id);
    let domain = canonical_domain(cookie.domain(), &host);
    if domain.is_empty() {
        return Err(Error::CreateWebview(
            "tauri-runtime-qt: cannot delete a cookie without a domain before the webview has a page URL"
                .into(),
        ));
    }
    let name = cookie.name().to_string();
    let request_path = match cookie.path() {
        Some(path) if !path.is_empty() => path.to_string(),
        _ => "/".to_string(),
    };

    ffi::qt_cookies_delete(view_ptr, &name, &domain, &request_path);

    let mut mirror = MIRROR.lock();
    if let Some(cookies) = mirror.cookies.get_mut(&webview_id) {
        cookies.retain(|(key_name, key_domain, key_path), _| {
            !(key_name == &name
                && domain_matches(key_domain.as_str(), &domain)
                && path_matches(key_path.as_str(), &request_path))
        });
    }
    Ok(())
}

/// Clears the browsing data Qt 6.11 can clear for this webview: the HTTP cache
/// and every cookie of the profile. The cookie mirror is emptied in the same
/// call — Qt's `deleteAllCookies()` is asynchronous and has no completion
/// signal, so waiting for the `cookieRemoved` events would race; the events
/// that do arrive find nothing to remove.
///
/// NOT cleared (Qt exposes no live-profile API for them): localStorage /
/// sessionStorage, IndexedDB, service-worker registrations and Cache Storage.
/// They live under the profile's storage directory, which this runtime does not
/// delete while the profile is alive.
pub(crate) fn clear_all_browsing_data(webview_id: u64) -> Result<()> {
    let view_ptr = view_ptr_of(webview_id)?;
    ffi::qt_cookies_clear_all(view_ptr);
    with_webview(webview_id, |cookies| cookies.clear())
}
