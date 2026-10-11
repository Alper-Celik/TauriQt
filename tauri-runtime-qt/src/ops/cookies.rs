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
//!   cookie, and the lowercased host for IP literals.
//!   `tauri-runtime-qt-core`'s `canonical_domain` reproduces that rule for
//!   optimistic writes.
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
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tauri_runtime::{Cookie, Error, Result};
use tauri_runtime_qt_core as qtcore;
use tauri_runtime_qt_core::cookies::{Key, Snapshot};

use crate::bridge::cookies::ffi;

// `Snapshot` (one cookie as the Qt store reported it, or as a write asked for
// it) and `Key` (name + domain + path — Chromium's identity for a cookie) live
// in the core crate, together with every matching rule used below.

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

/// The webview's current URL host, via the existing webview bridge ("" when the
/// webview has no page URL yet, e.g. before its first navigation).
fn current_host(webview_id: u64) -> String {
    qtcore::cookies::host_of_url(&crate::bridge::webview::ffi::qt_webview_url(webview_id))
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
        same_site: qtcore::cookies::same_site_from_code(same_site),
        expires_ms: (expires_ms >= 0).then_some(expires_ms),
    };
    let key = snapshot.key();
    let Some(cookies) = mirror.cookies.get_mut(&webview_id) else {
        return;
    };
    cookies.insert(key, snapshot);
}

/// A cookie left the store; removes the canonical key.
pub(crate) fn on_cookie_removed(webview_ptr: u64, name: &str, domain: &str, path: &str) {
    let mut mirror = MIRROR.lock();
    let Some(&webview_id) = mirror.ptr_to_id.get(&webview_ptr) else {
        return;
    };
    if let Some(cookies) = mirror.cookies.get_mut(&webview_id) {
        cookies.remove(&qtcore::cookies::key_of(name, domain, path));
    }
}

// ---- trait surface ----

/// Every cookie the mirror has observed for this webview, sorted by
/// (domain, path, name) for deterministic output. Expired entries can appear
/// here (an expiry-only change emits no event); `cookies_for_url` filters them.
pub(crate) fn cookies(webview_id: u64) -> Result<Vec<Cookie<'static>>> {
    with_webview(webview_id, |cookies| {
        let mut snapshots: Vec<&Snapshot> = cookies.values().collect();
        qtcore::cookies::sort_by_identity(&mut snapshots);
        snapshots
            .into_iter()
            .map(qtcore::cookies::to_public_cookie)
            .collect()
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
    let now = now_ms();
    with_webview(webview_id, |cookies| {
        let snapshots: Vec<&Snapshot> = cookies.values().collect();
        qtcore::cookies::cookies_for_url(&snapshots, url, now)
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
    let domain = qtcore::cookies::canonical_domain(cookie.domain(), &host);
    if domain.is_empty() {
        return Err(Error::CreateWebview(
            qtcore::cookies::missing_domain_message("set").into(),
        ));
    }
    let name = cookie.name().to_string();
    let value = cookie.value().to_string();
    let path = qtcore::cookies::request_path_of(&cookie);
    let secure = cookie.secure().unwrap_or(false);
    let http_only = cookie.http_only().unwrap_or(false);
    let same_site = cookie.same_site();
    let expires_ms = qtcore::cookies::expires_ms_of(&cookie, now_ms());

        crate::qtrace!(
        "[tauri-runtime-qt] set_cookie -> view {view_ptr:#x} name={name:?} value={value:?} domain={domain:?} path={path:?} secure={secure} http_only={http_only} same_site={} expires_ms={expires_ms}",
        qtcore::cookies::same_site_code(same_site)
    );
    ffi::qt_cookies_set(
        view_ptr,
        &name,
        &value,
        &domain,
        &path,
        secure,
        http_only,
        qtcore::cookies::same_site_code(same_site),
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
        cookies.insert(qtcore::cookies::key_of(&name, &domain, &path), snapshot);
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
    let domain = qtcore::cookies::canonical_domain(cookie.domain(), &host);
    if domain.is_empty() {
        return Err(Error::CreateWebview(
            qtcore::cookies::missing_domain_message("delete").into(),
        ));
    }
    let name = cookie.name().to_string();
    let request_path = qtcore::cookies::request_path_of(&cookie);

    ffi::qt_cookies_delete(view_ptr, &name, &domain, &request_path);

    let mut mirror = MIRROR.lock();
    if let Some(cookies) = mirror.cookies.get_mut(&webview_id) {
        cookies.retain(|(key_name, key_domain, key_path), _| {
            !(key_name == &name
                && qtcore::cookies::domain_matches(key_domain.as_str(), &domain)
                && qtcore::cookies::path_matches(key_path.as_str(), &request_path))
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
