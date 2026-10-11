// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Rust <-> Qt bridge for the runtime.
//!
//! The C++ side owns the `QApplication` singleton, the window/webview registries
//! and the custom URL scheme handlers; Rust drives it through this bridge.
//!
//! C++ translation units must include the cxx-generated header for this bridge
//! (it declares the `extern "Rust"` callbacks below with the correct linkage):
//! `#include "tauri-runtime-qt/src/qt.rs.h"`.

use tauri_runtime as rt;
use tauri_runtime::WindowEvent;

use crate::WINDOW_HANDLERS;

#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("qt_app.hpp");
        include!("qt_window.hpp");
        include!("qt_webview.hpp");
        include!("qt_scheme.hpp");

        // ---- application bootstrap (qt_app.cpp) ----
        /// Creates the `QApplication` and registers Tauri's URL schemes.
        /// Must be called on the main thread before any GUI object exists.
        /// Returns 0 on success, non-zero if an application already exists.
        fn qt_init(argc: i32) -> i32;

        /// Runs the Qt event loop; returns the process exit code.
        fn qt_run() -> i32;

        /// Version string of the Qt runtime actually in use (e.g. "6.11.2").
        fn qt_version() -> String;

        // ---- windows (qt_window.cpp) ----
        fn qt_window_create(
            label: &str,
            title: &str,
            width: i32,
            height: i32,
            visible: bool,
            decorations: bool,
        ) -> u64;
        fn qt_window_show(id: u64);
        fn qt_window_hide(id: u64);
        fn qt_window_close(id: u64);
        fn qt_window_set_title(id: u64, title: &str);
        fn qt_window_set_size(id: u64, width: i32, height: i32);
        fn qt_window_center(id: u64);
        fn qt_window_set_decorations(id: u64, decorations: bool);
        fn qt_window_set_maximized(id: u64, maximized: bool);
        fn qt_window_is_visible(id: u64) -> bool;
        fn qt_window_scale_factor(id: u64) -> f64;
        fn qt_window_winid(id: u64) -> u64;
        fn qt_window_attach_webview(window_id: u64, webview_native_id: u64);

        // ---- webviews (qt_webview.cpp) ----
        fn qt_webview_create(window_id: u64, url: &str) -> u64;
        fn qt_webview_native_handle(id: u64) -> u64;
        fn qt_webview_navigate(id: u64, url: &str);
        fn qt_webview_url(id: u64) -> String;
        fn qt_webview_reload(id: u64);
        fn qt_webview_eval(id: u64, script: &str);
        fn qt_webview_set_zoom(id: u64, factor: f64);
        fn qt_webview_set_background_color(id: u64, r: i32, g: i32, b: i32, a: i32);
        fn qt_webview_show(id: u64);
        fn qt_webview_hide(id: u64);
        fn qt_webview_set_bounds(id: u64, x: i32, y: i32, width: i32, height: i32);
        fn qt_webview_open_devtools(id: u64);
        fn qt_webview_close_devtools(id: u64);
        fn qt_webview_is_devtools_open(id: u64) -> bool;
        fn qt_webview_print(id: u64);
        fn qt_webview_close(id: u64);

        // ---- custom schemes (qt_scheme.cpp) ----
        /// Registers ipc/tauri/asset schemes; must run before the QApplication.
        fn qt_scheme_register_schemes();
        /// Installs the scheme handlers into the webview's profile.
        fn qt_scheme_install_handler(webview_id: u64);
    }

    extern "Rust" {
        /// Window events emitted from Qt: 0 = close requested, 1 = focused,
        /// 2 = unfocused, 3 = resized ("WxH"), 4 = moved ("x,y").
        fn qt_on_window_event(id: u64, kind: i32, payload: &str);

        /// Webview events: 1 = load started (url), 2 = load finished (url),
        /// 3 = url changed, 4 = title changed, 5 = load failed.
        fn qt_on_webview_event(id: u64, kind: i32, payload: &str);

        /// Custom-scheme request routed to Tauri's handler.
        /// Returns 1 when handled (response written through the out params),
        /// 0 when unhandled (the C++ side then fails the request).
        fn qt_scheme_request(
            scheme_kind: i32,
            method: &str,
            url: &str,
            headers: &str,
            body: &str,
        ) -> QtSchemeResponse;
    }

    /// Response produced for a custom-scheme request.
    struct QtSchemeResponse {
        /// 1 = handled (use the fields below), 0 = unhandled.
        handled: i32,
        status: i32,
        /// `\n`-separated `k: v` header list.
        headers: String,
        body: String,
    }
}

/// Window events emitted from Qt (see the bridge declaration for the kinds).
pub fn qt_on_window_event(id: u64, kind: i32, payload: &str) {
    let event = match kind {
        0 => {
            let (tx, _rx) = std::sync::mpsc::channel();
            WindowEvent::CloseRequested { signal_tx: tx }
        }
        1 => WindowEvent::Focused(true),
        2 => WindowEvent::Focused(false),
        3 => {
            let mut parts = payload.split('x');
            let (w, h) = (
                parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            );
            WindowEvent::Resized(rt::dpi::PhysicalSize::new(w, h))
        }
        4 => {
            let mut parts = payload.split(',');
            let (x, y) = (
                parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
                parts.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            );
            WindowEvent::Moved(rt::dpi::PhysicalPosition::new(x, y))
        }
        _ => return,
    };

    for (_, native, handler) in WINDOW_HANDLERS.lock().iter() {
        if *native == id {
            handler(&event);
        }
    }
}

/// Webview events emitted from Qt (see the bridge declaration for the kinds).
///
/// `tauri_runtime::window::WebviewEvent` only has `DragDrop` in 2.12, so load
/// state is currently only recorded (M2 will route it into the page-load
/// handler tauri sets through `WebviewAttributes`).
pub fn qt_on_webview_event(id: u64, kind: i32, payload: &str) {
    let _ = (id, kind, payload);
}

/// Custom-scheme request routed to Tauri's handler.
///
/// M1 wires this to the `ipc_handler` / `uri_scheme_protocols` tauri installed
/// on the pending webview; until then every request is reported unhandled.
pub fn qt_scheme_request(
    scheme_kind: i32,
    method: &str,
    url: &str,
    headers: &str,
    body: &str,
) -> QtSchemeResponse {
    let _ = (scheme_kind, method, url, headers, body);
    QtSchemeResponse {
        handled: 0,
        status: 404,
        headers: String::new(),
        body: String::new(),
    }
}
