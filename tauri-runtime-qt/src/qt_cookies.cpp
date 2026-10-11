// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#include "qt_cookies.hpp"

#include "tauri-runtime-qt/src/bridge/cookies.rs.h"

// Placeholder implementations: the cookies worker replaces these bodies with the
// QWebEngineCookieStore signal wiring and the set/delete/clear calls.

void qt_cookies_watch(std::uint64_t webview_ptr) {
    (void)webview_ptr;
}

void qt_cookies_set(std::uint64_t webview_ptr,
                    rust::Str name,
                    rust::Str value,
                    rust::Str domain,
                    rust::Str path,
                    bool secure,
                    bool http_only,
                    std::int32_t same_site,
                    std::int64_t expires_ms) {
    (void)webview_ptr;
    (void)name;
    (void)value;
    (void)domain;
    (void)path;
    (void)secure;
    (void)http_only;
    (void)same_site;
    (void)expires_ms;
}

void qt_cookies_delete(std::uint64_t webview_ptr,
                       rust::Str name,
                       rust::Str domain,
                       rust::Str path) {
    (void)webview_ptr;
    (void)name;
    (void)domain;
    (void)path;
}

void qt_cookies_clear_all(std::uint64_t webview_ptr) {
    (void)webview_ptr;
}
