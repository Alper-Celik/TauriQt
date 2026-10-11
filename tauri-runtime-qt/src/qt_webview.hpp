// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#pragma once

// Qt-backed webview registry.
//
// THREADING: every function declared here MUST be called from the Qt GUI thread
// (the thread that created the QApplication and runs QApplication::exec()).
// QWebEngineView / QWebEnginePage / QWebEngineProfile are all GUI-thread-affine;
// the Rust side marshals every call onto that thread (QMetaObject::invokeMethod
// with Qt::QueuedConnection). Calling anything here from a worker thread is a
// bug.
//
// The `extern "Rust"` callback `qt_on_webview_event(...)` used to deliver page
// events is declared by the cxx bridge (`tauri-runtime-qt/src/qt.rs.h`); it is
// NOT declared by hand here.

#include <rust/cxx.h>

#include <cstdint>

/// Creates a `QWebEngineView` (with its own default `QWebEnginePage`) for the
/// window identified by `window_id`, navigates it to `url` (empty = about:blank)
/// and returns a stable non-zero handle.
///
/// The view is created *unparented*: the caller embeds it with the handle from
/// `qt_webview_native_handle()` via the window agent
/// (`qt_window_attach_webview(window_id, qt_webview_native_handle(id))`), which
/// reparents the widget into the window and shows it.
uint64_t qt_webview_create(uint64_t window_id, rust::Str url);

/// Returns the `QWebEngineView*` of `id` as an opaque integer (the widget
/// address), or 0 for an unknown id/closed view. The window agent casts it back
/// to `QWebEngineView*` (`reinterpret_cast`) to reparent/attach it.
uint64_t qt_webview_native_handle(uint64_t id);

/// Navigates the webview to `url` (GET, like wry).
void qt_webview_navigate(uint64_t id, rust::Str url);

/// Current page URL as a string; empty for an unknown id.
rust::String qt_webview_url(uint64_t id);

/// Reloads the current page.
void qt_webview_reload(uint64_t id);

/// Runs `script` in the page's main world (fire-and-forget, no result).
void qt_webview_eval(uint64_t id, rust::Str script);

/// Sets the page zoom factor (Qt clamps to 0.25 .. 5.0).
void qt_webview_set_zoom(uint64_t id, double factor);

/// Sets the page background colour; components are 0-255.
void qt_webview_set_background_color(uint64_t id, int r, int g, int b, int a);

/// Shows / raises the view.
void qt_webview_show(uint64_t id);

/// Hides the view.
void qt_webview_hide(uint64_t id);

/// Moves/resizes the view inside its parent window (logical pixels, like Qt).
void qt_webview_set_bounds(uint64_t id, int x, int y, int w, int h);

/// Opens the inspector: creates a second `QWebEnginePage` + `QWebEngineView`
/// pair bound with `QWebEnginePage::setDevToolsPage()` and shows it. No-op if
/// already open or the id is unknown.
void qt_webview_open_devtools(uint64_t id);

/// Tears the devtools pair down (`setDevToolsPage(nullptr)` + delete).
void qt_webview_close_devtools(uint64_t id);

/// True while a devtools window exists and is visible.
bool qt_webview_is_devtools_open(uint64_t id);

/// Opens the print dialog for the page and prints on accept.
/// Requires user interaction (a modal QPrintDialog); in a headless/blocked
/// environment nothing is printed.
void qt_webview_print(uint64_t id);

/// Destroys the view (and its page / devtools pair) and unregisters the handle.
void qt_webview_close(uint64_t id);
