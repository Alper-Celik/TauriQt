// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#pragma once

#include <cstdint>

#include <rust/cxx.h>

// Cookie mirroring entry points (see src/bridge/cookies.rs). Handles are
// `QWebEngineView*` values (the webview native handle). Implemented in
// qt_cookies.cpp; the cookies worker owns both files.

/// Starts mirroring the cookie store of the given webview (idempotent).
void qt_cookies_watch(std::uint64_t webview_ptr);

/// Adds a cookie; `expires_ms` is Unix epoch milliseconds or -1 for a session
/// cookie, `same_site` is 0 = default, 1 = none, 2 = lax, 3 = strict.
void qt_cookies_set(std::uint64_t webview_ptr,
                    rust::Str name,
                    rust::Str value,
                    rust::Str domain,
                    rust::Str path,
                    bool secure,
                    bool http_only,
                    std::int32_t same_site,
                    std::int64_t expires_ms);

/// Deletes the cookie matching name/domain/path.
void qt_cookies_delete(std::uint64_t webview_ptr,
                       rust::Str name,
                       rust::Str domain,
                       rust::Str path);

/// Deletes every cookie of the profile.
void qt_cookies_clear_all(std::uint64_t webview_ptr);
