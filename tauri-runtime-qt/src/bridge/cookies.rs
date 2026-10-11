// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Cookie declarations (owned by the cookies worker).
//!
//! The runtime keeps its own copy of the profile's cookies: Qt's
//! `QWebEngineCookieStore` can only enumerate asynchronously and never signals
//! completion, so instead of blocking we mirror the store from its
//! `cookieAdded` / `cookieRemoved` signals and answer reads from that copy.
//!
//! C++ translation units include the cxx-generated header for this bridge:
//! `#include "tauri-runtime-qt/src/bridge/cookies.rs.h"`.

#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("qt_cookies.hpp");

        /// Starts mirroring the cookie store of the webview behind this handle
        /// (the `QWebEngineView*` value returned by `qt_webview_native_handle`).
        fn qt_cookies_watch(webview_ptr: u64);

        /// Adds a cookie to the store (`QWebEngineCookieStore::setCookie`).
        /// `expires_ms` is Unix epoch milliseconds, or -1 for a session cookie.
        /// `same_site`: 0 = default/unset, 1 = none, 2 = lax, 3 = strict.
        fn qt_cookies_set(
            webview_ptr: u64,
            name: &str,
            value: &str,
            domain: &str,
            path: &str,
            secure: bool,
            http_only: bool,
            same_site: i32,
            expires_ms: i64,
        );

        /// Deletes a cookie from the store.
        fn qt_cookies_delete(webview_ptr: u64, name: &str, domain: &str, path: &str);

        /// Deletes every cookie of the profile.
        fn qt_cookies_clear_all(webview_ptr: u64);
    }

    extern "Rust" {
        /// A cookie entered the store (including the initial load).
        #[allow(clippy::too_many_arguments)]
        fn qt_on_cookie_added(
            webview_ptr: u64,
            name: &str,
            value: &str,
            domain: &str,
            path: &str,
            secure: bool,
            http_only: bool,
            same_site: i32,
            expires_ms: i64,
        );

        /// A cookie left the store.
        fn qt_on_cookie_removed(webview_ptr: u64, name: &str, domain: &str, path: &str);
    }
}

/// Defined in the same module as the bridge, as cxx requires.
pub fn qt_on_cookie_added(
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
    crate::ops::cookies::on_cookie_added(
        webview_ptr,
        name,
        value,
        domain,
        path,
        secure,
        http_only,
        same_site,
        expires_ms,
    );
}

/// Defined in the same module as the bridge, as cxx requires.
pub fn qt_on_cookie_removed(webview_ptr: u64, name: &str, domain: &str, path: &str) {
    crate::ops::cookies::on_cookie_removed(webview_ptr, name, domain, path);
}
