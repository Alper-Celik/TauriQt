// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#pragma once

#include <cstdint>

#include <rust/cxx.h>

// Cookie mirroring entry points (see src/bridge/cookies.rs). Handles are
// `QWebEngineView*` values (the webview native handle). Implemented in
// qt_cookies.cpp; the cookies worker owns both files.
//
// THREADING: all four functions MUST be called on the Qt GUI thread — they
// touch `QWebEngineView` / `QWebEnginePage` / `QWebEngineProfile` /
// `QWebEngineCookieStore`, all of which are GUI-thread-affine. The Rust side
// only invokes them from tauri's dispatcher calls, which run on that thread.
//
// HANDLE CONTRACT: `webview_ptr` is the opaque address returned by
// `qt_webview_native_handle()` (a `QWebEngineView*`); 0 or an unknown pointer
// makes the call a no-op.
//
// `qt_cookies_watch` is the *only* source of mirror contents: the store reports
// every cookie through `cookieAdded` / `cookieRemoved`, including the ones
// loaded by the initial `loadAllCookies()`. See qt_cookies.cpp for what Qt does
// and does not preserve when a cookie goes through this API.

/// Starts mirroring the cookie store of the given webview (idempotent per
/// pointer). Connects `cookieAdded` / `cookieRemoved` (once per store — the
/// default profile's store is shared) and calls `loadAllCookies()` so the
/// mirror fills without waiting for a navigation. No result: a failure to watch
/// (unknown handle, no store) leaves the mirror empty instead of failing
/// webview creation.
void qt_cookies_watch(std::uint64_t webview_ptr);

/// Adds a cookie; `expires_ms` is Unix epoch milliseconds or -1 for a session
/// cookie, `same_site` is 0 = default, 1 = none, 2 = lax, 3 = strict.
///
/// A non-empty `domain` is used as the cookie's domain attribute (Chromium
/// makes it a domain cookie and prepends the dot when the host has a
/// registrable domain). An empty `domain` means host-only: the origin URL is
/// synthesised from the view's current host instead.
void qt_cookies_set(std::uint64_t webview_ptr,
                    rust::Str name,
                    rust::Str value,
                    rust::Str domain,
                    rust::Str path,
                    bool secure,
                    bool http_only,
                    std::int32_t same_site,
                    std::int64_t expires_ms);

/// Deletes the cookie matching name/domain/path. Chromium's deletion filter
/// uses the name plus a URL derived from the domain, so every path variant of
/// that name for the host is removed (the `path` argument is informational).
void qt_cookies_delete(std::uint64_t webview_ptr,
                       rust::Str name,
                       rust::Str domain,
                       rust::Str path);

/// Deletes every cookie of the profile and clears the HTTP cache. Asynchronous
/// fire-and-forget (no completion signal). localStorage, IndexedDB and service
/// workers are NOT cleared — see the implementation comment.
void qt_cookies_clear_all(std::uint64_t webview_ptr);
