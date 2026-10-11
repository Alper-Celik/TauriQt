// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Cookie matching rules and the mirror's bookkeeping — the half of
//! `ops::cookies` that never touches Qt.
//!
//! The runtime keeps the `MIRROR` static, the FFI calls and the trace lines;
//! what lives here is every decision the mirror makes: Chromium's canonical
//! domain form, the host/path/secure/expiry rules a request applies, the
//! same-site code the C++ bridge speaks, and the snapshot → public `Cookie`
//! conversion. See `tauri-runtime-qt/src/ops/cookies.rs` for *why* the mirror
//! exists (Qt's cookie store can only be enumerated asynchronously).

use std::net::IpAddr;

use cookie::SameSite;
use tauri_runtime::Cookie;
use url::Url;

/// One cookie as the Qt store reported it (or as a write asked for it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub name: String,
    pub value: String,
    /// Chromium's canonical domain (see [`canonical_domain`]).
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
    /// Unix epoch milliseconds; `None` for a session cookie.
    pub expires_ms: Option<i64>,
}

/// Chromium identifies a cookie by name + domain + path; the mirror does too.
pub type Key = (String, String, String);

/// The host of a URL as the matching rules above expect it: lower-cased, and
/// empty for a URL that does not parse (a webview without a page URL yet).
pub fn host_of_url(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .unwrap_or_default()
}

/// The key a cookie is stored under (and a removal event carries): the domain
/// is normalised to lower case, the name and path are kept as they came.
pub fn key_of(name: &str, domain: &str, path: &str) -> Key {
    (
        name.to_string(),
        domain.to_ascii_lowercase(),
        path.to_string(),
    )
}

impl Snapshot {
    /// The key this snapshot is stored under — see [`key_of`].
    pub fn key(&self) -> Key {
        key_of(&self.name, &self.domain, &self.path)
    }
}

/// The domain form Chromium (and therefore the Qt events) uses for a cookie
/// written with `requested`:
///
/// * no domain → host-only for the webview's current host (`fallback_host`);
/// * a domain whose host has a registrable domain → `.domain` (Chromium
///   prepends the dot, matching RFC 6265's domain cookie);
/// * a bare single-label host (`localhost`) or an IP literal → kept as-is,
///   which Chromium stores as a host-only cookie.
///
/// `requested` is regularised first: the leading dot is dropped, the string is
/// lowercased.
pub fn canonical_domain(requested: Option<&str>, fallback_host: &str) -> String {
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
pub fn domain_matches(cookie_domain: &str, host: &str) -> bool {
    match cookie_domain.strip_prefix('.') {
        Some(suffix) => host == suffix || host.ends_with(cookie_domain),
        None => host == cookie_domain,
    }
}

/// The cookie's path must be a prefix of the request path (empty → "/").
pub fn path_matches(cookie_path: &str, request_path: &str) -> bool {
    let cookie_path = if cookie_path.is_empty() {
        "/"
    } else {
        cookie_path
    };
    request_path.starts_with(cookie_path)
}

/// The `same_site` code the C++ bridge expects (0 = default/absent).
pub fn same_site_code(same_site: Option<SameSite>) -> i32 {
    match same_site {
        Some(SameSite::None) => 1,
        Some(SameSite::Lax) => 2,
        Some(SameSite::Strict) => 3,
        None => 0,
    }
}

/// Inverse of [`same_site_code`] for the Qt side of the bridge. An unknown code
/// (a newer Qt) is treated as "attribute absent" instead of guessing.
pub fn same_site_from_code(code: i32) -> Option<SameSite> {
    match code {
        1 => Some(SameSite::None),
        2 => Some(SameSite::Lax),
        3 => Some(SameSite::Strict),
        _ => None,
    }
}

/// The path a write/delete attaches to: the cookie's own path, or Qt's default
/// `/` when it carries none (or an empty one).
pub fn request_path_of(cookie: &Cookie<'_>) -> String {
    match cookie.path() {
        Some(path) if !path.is_empty() => path.to_string(),
        _ => "/".to_string(),
    }
}

/// The message reported for a write/delete that carries no domain while the
/// webview has no page URL yet (i.e. there is no host to be host-only for).
/// `verb` is `"set"` or `"delete"`; the runtime wraps it in its error type.
pub fn missing_domain_message(verb: &str) -> String {
    format!("tauri-runtime-qt: cannot {verb} a cookie without a domain before the webview has a page URL")
}

/// Expiry of a cookie about to be written, as Unix epoch milliseconds
/// (`-1` = session cookie). `max_age` wins when `expires` is absent, matching
/// HTTP semantics; `Max-Age: 0` therefore means "expire immediately".
///
/// `now_ms` is the clock the relative `max_age` is resolved against — passed in
/// so the result is a pure function of its inputs (the runtime passes
/// `now_ms()`, tests pass a fixed instant).
pub fn expires_ms_of(cookie: &Cookie<'_>, now_ms: i64) -> i64 {
    if let Some(expires) = cookie.expires_datetime() {
        return expires.unix_timestamp() * 1_000 + i64::from(expires.millisecond());
    }
    match cookie.max_age() {
        Some(max_age) => {
            let delta = max_age.whole_milliseconds();
            let delta = delta.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
            now_ms.saturating_add(delta)
        }
        None => -1,
    }
}

/// Snapshot → the trait's public cookie type.
pub fn to_public_cookie(snapshot: &Snapshot) -> Cookie<'static> {
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

/// The mirror's order: by (domain, path, name), so the output is deterministic
/// whatever order the Qt events arrived in.
pub fn sort_by_identity(snapshots: &mut [&Snapshot]) {
    snapshots.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
}

/// Whether a request applies this cookie:
///
/// * a host-only cookie (domain without a leading dot) matches the exact host;
/// * a domain cookie (leading dot) matches that host and every subdomain;
/// * the cookie's path must be a prefix of the request path (empty → "/");
/// * `Secure` cookies apply to `https`/`wss` requests only;
/// * expired cookies are skipped.
pub fn cookie_matches_request(
    snapshot: &Snapshot,
    host: &str,
    request_path: &str,
    secure_request: bool,
    now_ms: i64,
) -> bool {
    domain_matches(snapshot.domain.as_str(), host)
        && path_matches(snapshot.path.as_str(), request_path)
        && (!snapshot.secure || secure_request)
        && snapshot
            .expires_ms
            .is_none_or(|expires_ms| expires_ms > now_ms)
}

/// The cookies a request to `url` would carry, in [`sort_by_identity`] order.
///
/// Deliberate difference from Chromium: the scheme gate (Chromium only answers
/// for `http`/`https`/`ws`/`wss`) is not applied, so callers can ask about the
/// runtime's own `tauri://` pages; host and path still decide the match.
pub fn cookies_for_url(snapshots: &[&Snapshot], url: &Url, now_ms: i64) -> Vec<Cookie<'static>> {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let request_path = if url.path().is_empty() {
        "/"
    } else {
        url.path()
    };
    let secure_request = matches!(url.scheme(), "https" | "wss");
    let mut matched: Vec<&Snapshot> = snapshots
        .iter()
        .copied()
        .filter(|cookie| {
            cookie_matches_request(cookie, &host, request_path, secure_request, now_ms)
        })
        .collect();
    sort_by_identity(&mut matched);
    matched.into_iter().map(to_public_cookie).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cookie::time::{Duration, OffsetDateTime};

    fn snapshot(name: &str, domain: &str, path: &str) -> Snapshot {
        Snapshot {
            name: name.to_string(),
            value: format!("{name}-value"),
            domain: domain.to_string(),
            path: path.to_string(),
            secure: false,
            http_only: false,
            same_site: None,
            expires_ms: None,
        }
    }

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect("valid url")
    }

    fn names_for(mirror: &[Snapshot], raw_url: &str, now_ms: i64) -> Vec<String> {
        let refs: Vec<&Snapshot> = mirror.iter().collect();
        cookies_for_url(&refs, &url(raw_url), now_ms)
            .iter()
            .map(|cookie| cookie.name().to_string())
            .collect()
    }

    #[test]
    fn canonical_domain_falls_back_to_the_current_host() {
        assert_eq!(canonical_domain(None, "Example.COM"), "example.com");
        assert_eq!(canonical_domain(Some(""), "example.com"), "example.com");
        assert_eq!(canonical_domain(Some("   "), "example.com"), "example.com");
    }

    #[test]
    fn canonical_domain_dots_and_lowercases_registrable_domains() {
        assert_eq!(
            canonical_domain(Some("Example.com"), "host"),
            ".example.com"
        );
        assert_eq!(
            canonical_domain(Some(".example.com"), "host"),
            ".example.com"
        );
        assert_eq!(
            canonical_domain(Some("sub.example.com"), "host"),
            ".sub.example.com"
        );
    }

    #[test]
    fn canonical_domain_keeps_single_label_and_ip_hosts_bare() {
        assert_eq!(canonical_domain(Some("localhost"), "host"), "localhost");
        assert_eq!(canonical_domain(Some("127.0.0.1"), "host"), "127.0.0.1");
        assert_eq!(canonical_domain(Some("::1"), "host"), "::1");
        assert_eq!(canonical_domain(Some("2001:DB8::1"), "host"), "2001:db8::1");
    }

    #[test]
    fn host_only_cookies_match_the_exact_host_only() {
        assert!(domain_matches("example.com", "example.com"));
        assert!(!domain_matches("example.com", "sub.example.com"));
        assert!(!domain_matches("example.com", "example.com.evil"));
    }

    #[test]
    fn domain_cookies_match_the_domain_and_every_subdomain() {
        assert!(domain_matches(".example.com", "example.com"));
        assert!(domain_matches(".example.com", "sub.example.com"));
        assert!(domain_matches(".example.com", "a.b.example.com"));
        assert!(!domain_matches(".example.com", "example.com.evil"));
        assert!(!domain_matches(".example.com", "notexample.com"));
    }

    #[test]
    fn paths_match_as_prefixes_with_empty_meaning_root() {
        assert!(path_matches("/a", "/a"));
        assert!(path_matches("/a", "/a/b"));
        assert!(!path_matches("/a/b", "/a"));
        assert!(path_matches("", "/anything"));
        assert!(path_matches("/", "/"));
    }

    #[test]
    fn same_site_codes_round_trip() {
        for same_site in [
            Some(SameSite::None),
            Some(SameSite::Lax),
            Some(SameSite::Strict),
            None,
        ] {
            assert_eq!(same_site_from_code(same_site_code(same_site)), same_site);
        }
        assert_eq!(same_site_code(Some(SameSite::None)), 1);
        assert_eq!(same_site_code(Some(SameSite::Lax)), 2);
        assert_eq!(same_site_code(Some(SameSite::Strict)), 3);
        assert_eq!(same_site_code(None), 0);
    }

    #[test]
    fn unknown_same_site_codes_read_as_absent() {
        assert_eq!(same_site_from_code(-1), None);
        assert_eq!(same_site_from_code(0), None);
        assert_eq!(same_site_from_code(4), None);
    }

    #[test]
    fn host_of_url_lowercases_and_falls_back_to_empty() {
        assert_eq!(host_of_url("https://Example.COM/a/b"), "example.com");
        assert_eq!(host_of_url("tauri://localhost/index.html"), "localhost");
        // Not a URL, no host, or nothing at all: no host to be host-only for.
        assert_eq!(host_of_url("not a url"), "");
        assert_eq!(host_of_url(""), "");
        assert_eq!(host_of_url("data:text/plain,hi"), "");
    }

    #[test]
    fn keys_normalise_the_domain_to_lower_case() {
        let snapshot = snapshot("sid", ".Example.COM", "/app");
        assert_eq!(
            snapshot.key(),
            (
                "sid".to_string(),
                ".example.com".to_string(),
                "/app".to_string()
            )
        );
        assert_eq!(key_of("sid", ".Example.COM", "/app"), snapshot.key());
    }

    #[test]
    fn request_path_defaults_to_root() {
        assert_eq!(request_path_of(&Cookie::build(("a", "b")).build()), "/");
        assert_eq!(
            request_path_of(&Cookie::build(("a", "b")).path("").build()),
            "/"
        );
        assert_eq!(
            request_path_of(&Cookie::build(("a", "b")).path("/x").build()),
            "/x"
        );
    }

    #[test]
    fn missing_domain_messages_are_stable() {
        assert_eq!(
            missing_domain_message("set"),
            "tauri-runtime-qt: cannot set a cookie without a domain before the webview has a page URL"
        );
        assert_eq!(
            missing_domain_message("delete"),
            "tauri-runtime-qt: cannot delete a cookie without a domain before the webview has a page URL"
        );
    }

    #[test]
    fn expires_wins_over_max_age() {
        let cookie = Cookie::build(("a", "b"))
            .expires(OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid instant"))
            .max_age(Duration::seconds(60))
            .build();
        assert_eq!(expires_ms_of(&cookie, 0), 1_700_000_000_000);
    }

    #[test]
    fn expires_keeps_the_millisecond_component() {
        let expires = OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid instant")
            + Duration::milliseconds(250);
        let cookie = Cookie::build(("a", "b")).expires(expires).build();
        assert_eq!(expires_ms_of(&cookie, 0), 1_700_000_000_250);
    }

    #[test]
    fn max_age_is_relative_to_now() {
        let cookie = Cookie::build(("a", "b"))
            .max_age(Duration::seconds(60))
            .build();
        assert_eq!(expires_ms_of(&cookie, 1_000), 61_000);
    }

    #[test]
    fn max_age_zero_expires_immediately() {
        let cookie = Cookie::build(("a", "b"))
            .max_age(Duration::seconds(0))
            .build();
        assert_eq!(expires_ms_of(&cookie, 1_000), 1_000);
    }

    #[test]
    fn no_expiry_is_a_session_cookie() {
        let cookie = Cookie::build(("a", "b")).build();
        assert_eq!(expires_ms_of(&cookie, 1_000), -1);
    }

    #[test]
    fn absurd_max_age_saturates_instead_of_overflowing() {
        let cookie = Cookie::build(("a", "b"))
            .max_age(Duration::seconds(i64::MAX))
            .build();
        assert_eq!(expires_ms_of(&cookie, 1_000), i64::MAX);
    }

    #[test]
    fn snapshot_converts_to_the_public_cookie_type() {
        let snapshot = Snapshot {
            name: "sid".to_string(),
            value: "v".to_string(),
            domain: ".example.com".to_string(),
            path: "/app".to_string(),
            secure: true,
            http_only: true,
            same_site: Some(SameSite::Lax),
            expires_ms: Some(1_700_000_000_000),
        };

        let cookie = to_public_cookie(&snapshot);
        assert_eq!(cookie.name(), "sid");
        assert_eq!(cookie.value(), "v");
        // `cookie::Cookie::domain()` strips the leading dot of a domain cookie
        // (as `domain_raw()` does), so the dot never reaches a caller.
        assert_eq!(cookie.domain(), Some("example.com"));
        assert_eq!(cookie.path(), Some("/app"));
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(
            cookie.expires_datetime(),
            Some(OffsetDateTime::from_unix_timestamp(1_700_000_000).expect("valid instant"))
        );
    }

    #[test]
    fn session_snapshot_has_no_expiry() {
        let cookie = to_public_cookie(&snapshot("sid", "example.com", "/"));
        assert_eq!(cookie.expires_datetime(), None);
        assert_eq!(cookie.secure(), None);
        assert_eq!(cookie.http_only(), None);
        assert_eq!(cookie.same_site(), None);
    }

    #[test]
    fn unrepresentable_expiry_is_dropped_instead_of_panicking() {
        let mut snapshot = snapshot("sid", "example.com", "/");
        snapshot.expires_ms = Some(i64::MAX);
        assert_eq!(to_public_cookie(&snapshot).expires_datetime(), None);
    }

    #[test]
    fn sorting_is_by_domain_then_path_then_name() {
        let mirror = [
            snapshot("b", "example.com", "/"),
            snapshot("a", ".example.com", "/"),
            snapshot("c", "example.com", "/a"),
        ];
        let mut refs: Vec<&Snapshot> = mirror.iter().collect();
        sort_by_identity(&mut refs);
        let names: Vec<&str> = refs.iter().map(|snapshot| snapshot.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }

    #[test]
    fn cookies_for_url_applies_domain_path_secure_and_expiry_rules() {
        let now = 1_000_000;
        let mut mirror = vec![
            snapshot("host_only", "example.com", "/"),
            snapshot("domain", ".example.com", "/"),
            snapshot("sub_path", "example.com", "/app"),
            snapshot("expired", "example.com", "/"),
            snapshot("secure", "example.com", "/"),
            snapshot("other_host", "example.org", "/"),
        ];
        mirror[3].expires_ms = Some(now - 1);
        mirror[4].secure = true;

        assert_eq!(
            names_for(&mirror, "http://example.com/app/page", now),
            ["domain", "host_only", "sub_path"]
        );
        assert_eq!(
            names_for(&mirror, "https://example.com/app/page", now),
            ["domain", "host_only", "secure", "sub_path"]
        );
        // A domain cookie reaches subdomains; the host-only ones do not.
        assert_eq!(
            names_for(&mirror, "https://sub.example.com/", now),
            ["domain"]
        );
    }

    #[test]
    fn cookies_for_url_skips_expired_cookies_except_at_the_exact_instant() {
        let now = 1_000;
        let mut mirror = vec![
            snapshot("gone", "example.com", "/"),
            snapshot("live", "example.com", "/"),
        ];
        mirror[0].expires_ms = Some(now);
        mirror[1].expires_ms = Some(now + 1);

        assert_eq!(names_for(&mirror, "http://example.com/", now), ["live"]);
    }

    #[test]
    fn cookies_for_url_ignores_the_scheme_gate() {
        // `tauri://` pages are what this runtime serves; host and path still
        // decide the match, only the secure gate is scheme-dependent.
        let mirror = vec![snapshot("host_only", "example.com", "/")];
        assert_eq!(names_for(&mirror, "tauri://example.com/", 0), ["host_only"]);
        assert_eq!(names_for(&mirror, "ws://example.com/", 0), ["host_only"]);
    }
}
