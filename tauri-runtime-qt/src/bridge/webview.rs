// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Webview declarations (owned by the webview worker).
//!
//! C++ translation units include the cxx-generated header for this bridge:
//! `#include "tauri-runtime-qt/src/bridge/webview.rs.h"`.

#[cxx::bridge]
pub mod ffi {
    /// Live geometry of a webview widget, in **physical (device) pixels**.
    ///
    /// Qt reports widget geometry in device-independent pixels, so
    /// `qt_webview.cpp` multiplies by the widget's `devicePixelRatioF()` before
    /// filling this in. `ok == false` (all other fields zero) means the handle
    /// is unknown: Rust turns that into an `Err` instead of inventing numbers.
    struct QtWebviewGeometry {
        ok: bool,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
    }

    unsafe extern "C++" {
        include!("qt_webview.hpp");

        // ---- webviews (qt_webview.cpp) ----
        fn qt_webview_create(window_id: u64, url: &str) -> u64;
        /// Injects a tauri initialization script at document creation.
        fn qt_webview_add_init_script(id: u64, script: &str, main_frame_only: bool);
        fn qt_webview_native_handle(id: u64) -> u64;
        fn qt_webview_navigate(id: u64, url: &str);
        fn qt_webview_url(id: u64) -> String;
        fn qt_webview_reload(id: u64);
        fn qt_webview_eval(id: u64, script: &str);
        fn qt_webview_set_zoom(id: u64, factor: f64);
        fn qt_webview_set_background_color(id: u64, r: i32, g: i32, b: i32, a: i32);
        fn qt_webview_show(id: u64);
        fn qt_webview_hide(id: u64);
        fn qt_webview_open_devtools(id: u64);
        fn qt_webview_close_devtools(id: u64);
        fn qt_webview_is_devtools_open(id: u64) -> bool;
        fn qt_webview_print(id: u64);
        fn qt_webview_close(id: u64);

        // ---- geometry / focus / reparenting ----
        /// Live widget geometry in physical pixels (see `QtWebviewGeometry`).
        fn qt_webview_geometry(id: u64) -> QtWebviewGeometry;
        /// Resizes the widget; `physical == true` means the caller passed
        /// physical pixels, which are divided by the widget's devicePixelRatio
        /// (`false` = Qt device-independent pixels, used as-is). `false` as a
        /// return value means the handle is unknown. The first positional call
        /// takes the view out of the window's layout (see `ops/webview.rs`).
        fn qt_webview_set_size(id: u64, width: i32, height: i32, physical: bool) -> bool;
        /// Moves the widget relative to its parent window; same unit rule as
        /// `qt_webview_set_size`.
        fn qt_webview_set_position(id: u64, x: i32, y: i32, physical: bool) -> bool;
        /// `QWebEngineView::setFocus()`.
        fn qt_webview_set_focus(id: u64) -> bool;
        /// Runs `script` and delivers the result as a JSON string to
        /// `qt_on_eval_result(request_id, result)`.
        fn qt_webview_eval_with_callback(id: u64, request_id: u64, script: &str) -> bool;
        /// Reparents the view into the window identified by the
        /// `qt_window_create` handle `window_native`.
        fn qt_webview_reparent(id: u64, window_native: u64) -> bool;
        /// Clears the browsing data Qt can clear (HTTP cache, cookies, visited
        /// links); see the C++ implementation for what cannot be cleared.
        fn qt_webview_clear_browsing_data(id: u64) -> bool;
    }

    extern "Rust" {
        /// Result of a `QWebEnginePage::runJavaScript` started by
        /// `qt_webview_eval_with_callback`, as a JSON string (`null` for an
        /// undefined/unrepresentable value). Called on the Qt GUI thread;
        /// `request_id` is the id Rust allocated. Unknown/expired ids are
        /// dropped.
        fn qt_on_eval_result(request_id: u64, result: &str);

        /// Diagnostic trigger: the first webview calls this once when its first
        /// page load finishes. Rust runs the `TAURIQT_EVAL_DEBUG` smoke test
        /// (`document.title = 'eval-ok'` through `eval_with_callback`) and does
        /// nothing when the environment variable is unset.
        fn qt_webview_on_first_load(webview_id: u64);
    }
}

/// Delivers a `runJavaScript` result to the webview area.
///
/// cxx requires an `extern "Rust"` function to be defined in the module that
/// declares the bridge, so this stays a thin forwarder and the pending-request
/// registry lives with the rest of the webview logic in `crate::ops::webview`.
pub fn qt_on_eval_result(request_id: u64, result: &str) {
    crate::ops::webview::on_eval_result(request_id, result);
}

/// `TAURIQT_EVAL_DEBUG` smoke-test trigger; see
/// `crate::ops::webview::debug_hook_first_load`.
pub fn qt_webview_on_first_load(webview_id: u64) {
    crate::ops::webview::debug_hook_first_load(webview_id);
}
