// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#pragma once

#include <cstdint>

class QWebEngineView;

/// Registers the `ipc`, `tauri` and `asset` custom URL schemes.
///
/// QtWebEngine requires scheme registration at application startup, before any
/// WebEngine (and in practice before any `QApplication`) object is created, so
/// this is called from `qt_init()`. Calling it more than once is a no-op.
void qt_scheme_register_schemes();

/// Associates a webview id with the `QWebEngineView` backing it, so that
/// `qt_scheme_install_handler()` can find the `QWebEngineProfile` to install the
/// scheme handlers on. The webview layer calls this right after it creates the
/// view. The view pointer is stored weakly (`QPointer`).
void qt_scheme_bind_webview(std::uint64_t webview_id, QWebEngineView* view);

/// Installs the `ipc`, `tauri` and `asset` `QWebEngineUrlSchemeHandler`s into the
/// profile used by the webview previously bound with `qt_scheme_bind_webview()`.
/// Must be called on the GUI thread after the webview exists. Requests are routed
/// to Rust through the `qt_scheme_request` callback declared by the cxx bridge.
void qt_scheme_install_handler(std::uint64_t webview_id);
