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
//! Semantics for callers: the mirror is an **observed snapshot**. Cookies whose
//! only change was server-side expiry, or cookies written before the store was
//! watched, appear only after the initial load/navigation.
//!
//! Contract: `register_webview` is called once per webview right after creation;
//! `on_cookie_added` / `on_cookie_removed` are called from the Qt signals.

#![allow(unused_variables)]

use tauri_runtime::{Cookie, Result};

use super::unimplemented;

/// Starts mirroring the cookie store of the given webview.
///
/// `webview_ptr` is the value from `qt_webview_native_handle`. Never returns an
/// error because it runs inside webview creation; a failure to watch is logged
/// (the worker's implementation decides how) and leaves the mirror empty.
pub(crate) fn register_webview(webview_id: u64, webview_ptr: u64) {
    // TODO(cookies worker): call `bridge::cookies::ffi::qt_cookies_watch(webview_ptr)`
    // and remember the webview for later lookups.
}

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
    // TODO(cookies worker): insert into the mirror.
}

pub(crate) fn on_cookie_removed(webview_ptr: u64, name: &str, domain: &str, path: &str) {
    // TODO(cookies worker): remove from the mirror.
}

// ---- trait surface ----

pub(crate) fn cookies_for_url(webview_id: u64, url: &url::Url) -> Result<Vec<Cookie<'static>>> {
    Err(unimplemented("WebviewDispatch::cookies_for_url"))
}

pub(crate) fn cookies(webview_id: u64) -> Result<Vec<Cookie<'static>>> {
    Err(unimplemented("WebviewDispatch::cookies"))
}

pub(crate) fn set_cookie(webview_id: u64, cookie: cookie::Cookie<'_>) -> Result<()> {
    Err(unimplemented("WebviewDispatch::set_cookie"))
}

pub(crate) fn delete_cookie(webview_id: u64, cookie: cookie::Cookie<'_>) -> Result<()> {
    Err(unimplemented("WebviewDispatch::delete_cookie"))
}

/// `clear_all_browsing_data` is broader than cookies (cache, storage); the
/// worker implements what Qt 6.11 offers and documents the rest.
pub(crate) fn clear_all_browsing_data(webview_id: u64) -> Result<()> {
    Err(unimplemented("WebviewDispatch::clear_all_browsing_data"))
}
