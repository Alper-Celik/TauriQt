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
// The `extern "Rust"` callbacks `qt_on_webview_event(...)` (page events, from
// `tauri-runtime-qt/src/qt.rs.h`) and `qt_on_eval_result(...)` /
// `qt_webview_on_first_load(...)` (this bridge) are declared by the cxx bridge
// headers; they are NOT declared by hand here.
//
// GEOMETRY: `qt_webview_geometry()` reports the live widget geometry in
// physical pixels (Qt reports device-independent pixels, so the widget's
// `devicePixelRatioF()` is applied). `qt_webview_set_size()/set_position()`
// place the widget explicitly, which takes it out of the window's
// layout; see the "geometry ownership" comment in `qt_webview.cpp`.

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

/// Registers an initialization script for the view's page (tauri's `core.js`,
/// `ipc.js`, ...). The script is injected at `DocumentCreation` into `MainWorld`
/// on the *next* navigation, so it MUST be added before the first `load()` (i.e.
/// before `qt_webview_navigate` / a non-empty URL in `qt_webview_create`).
/// `main_frame_only == true` maps to `runsOnSubFrames(false)`; each call gets a
/// unique script name. Unknown id -> no-op.
void qt_webview_add_init_script(uint64_t id, rust::Str script, bool main_frame_only);

/// Sets the page zoom factor (Qt clamps to 0.25 .. 5.0).
void qt_webview_set_zoom(uint64_t id, double factor);

/// Sets the page background colour; components are 0-255.
void qt_webview_set_background_color(uint64_t id, int r, int g, int b, int a);

/// Shows / raises the view.
void qt_webview_show(uint64_t id);

/// Hides the view.
void qt_webview_hide(uint64_t id);

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

/// Live geometry of the webview widget in **physical (device) pixels**
/// (`QWidget::geometry()` x `devicePixelRatioF()`). A `false` `ok` field — with
/// every other field zero — means the id is unknown.
///
/// Defined by the cxx bridge `tauri-runtime-qt/src/bridge/webview.rs`.
struct QtWebviewGeometry;

/// See `struct QtWebviewGeometry`.
struct QtWebviewGeometry qt_webview_geometry(uint64_t id);

/// Resizes the widget. `physical == true` means `width`/`height` are physical
/// (device) pixels and are divided by the widget's `devicePixelRatioF()`;
/// `false` means Qt device-independent pixels, used as-is.
///
/// This takes the view out of its parent window's layout (see the "geometry
/// ownership" comment in `qt_webview.cpp`). Returns false for an unknown id.
bool qt_webview_set_size(uint64_t id, int width, int height, bool physical);

/// Moves the widget relative to its parent window; same unit rule as
/// `qt_webview_set_size`. Returns false for an unknown id.
bool qt_webview_set_position(uint64_t id, int x, int y, bool physical);

/// `QWebEngineView::setFocus()`. Returns false for an unknown id.
bool qt_webview_set_focus(uint64_t id);

/// Runs `script` in the page's main world and delivers the result to Rust as a
/// JSON string through the `extern "Rust"` callback
/// `qt_on_eval_result(request_id, result)`. `request_id` is allocated by Rust
/// (one pending-request registry entry) and echoed back verbatim. An undefined
/// or non-JSON JS value is reported as `null`.
/// Returns false for an unknown id (no callback is scheduled then).
bool qt_webview_eval_with_callback(uint64_t id, uint64_t request_id, rust::Str script);

/// Reparents the view into the top-level window identified by `window_native`
/// (the handle returned by `qt_window_create()`) by going through the window
/// agent's `qt_window_attach_webview()`, which re-embeds the widget into the
/// target window's client area and shows it. Returns false for an unknown
/// webview id or an unknown window handle.
bool qt_webview_reparent(uint64_t id, uint64_t window_native);

/// Clears the browsing data Qt 6.11 can clear for this view's profile: the HTTP
/// cache (`QWebEngineProfile::clearHttpCache()`), all cookies
/// (`cookieStore()->deleteAllCookies()`) and the visited-links list. Fire and
/// forget — both cache clearing and cookie deletion are asynchronous in Qt and
/// have no completion signal. localStorage, IndexedDB and service workers are
/// NOT cleared (Qt exposes no API for them on a live profile); the full list of
/// gaps is in the implementation comment in `qt_webview.cpp`.
/// Returns false for an unknown id.
bool qt_webview_clear_browsing_data(uint64_t id);
