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

use parking_lot::Mutex;
use tauri_runtime as rt;
use tauri_runtime::window::WindowEvent;

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

        /// Custom-scheme request routed to the handler tauri registered for the
        /// webview. `scheme_kind`: 0 = ipc, 1 = tauri, 2 = asset. `headers` is a
        /// newline-separated `k: v` list, and the response carries the same
        /// encoding back.
        fn qt_scheme_request(
            webview_id: u64,
            scheme_kind: i32,
            method: &str,
            url: &str,
            headers: &str,
            body: &str,
        ) -> QtSchemeResponse;
    }

    /// Response produced for a custom-scheme request.
    struct QtSchemeResponse {
        /// 1 = handled (the fields below are valid), 0 = unhandled.
        handled: i32,
        status: i32,
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

    for (_, native, handler) in crate::WINDOW_HANDLERS.lock().iter() {
        if *native == id {
            handler(&event);
        }
    }
}

/// Webview events emitted from Qt (see the bridge declaration for the kinds).
///
/// `tauri_runtime::window::WebviewEvent` only has `DragDrop` in 2.12; page-load
/// and title events must go through the handlers tauri puts on `PendingWebview`.
pub fn qt_on_webview_event(id: u64, kind: i32, payload: &str) {
    let _ = (id, kind, payload);
}

fn unhandled() -> ffi::QtSchemeResponse {
    ffi::QtSchemeResponse {
        handled: 0,
        status: 404,
        headers: String::new(),
        body: String::new(),
    }
}

/// Custom-scheme request routed to the handler tauri registered for this webview.
pub fn qt_scheme_request(
    webview_id: u64,
    scheme_kind: i32,
    method: &str,
    url: &str,
    headers: &str,
    body: &str,
) -> ffi::QtSchemeResponse {
    let scheme = match scheme_kind {
        0 => "ipc",
        1 => "tauri",
        2 => "asset",
        _ => return unhandled(),
    };

    let mut request = http::Request::builder().method(method).uri(url);
    for line in headers.lines() {
        if let Some((name, value)) = line.split_once(": ") {
            request = request.header(name.trim(), value.trim());
        }
    }
    let Ok(request) = request.body(body.as_bytes().to_vec()) else {
        return unhandled();
    };

    let slot: std::sync::Arc<Mutex<Option<http::Response<std::borrow::Cow<'static, [u8]>>>>> =
        std::sync::Arc::new(Mutex::new(None));
    let captured = std::sync::Arc::clone(&slot);

    {
        let guard = crate::SCHEME_PROTOCOLS.lock();
        let Some(entry) = guard.iter().find(|(id, _)| *id == webview_id) else {
            return unhandled();
        };
        let Some(handler) = entry.1.get(scheme) else {
            return unhandled();
        };
        handler(
            scheme,
            request,
            Box::new(move |response: http::Response<std::borrow::Cow<'static, [u8]>>| {
                *captured.lock() = Some(response);
            }),
        );
    }

    let Some(response) = slot.lock().take() else {
        // tauri resolved the invoke asynchronously; the synchronous bridge
        // cannot wait for it (documented limitation, see PLAN.md M1).
        return unhandled();
    };

    let status = response.status().as_u16() as i32;
    let mut out_headers = String::new();
    for (name, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            out_headers.push_str(name.as_str());
            out_headers.push_str(": ");
            out_headers.push_str(&value.replace(['\r', '\n'], ""));
            out_headers.push('\n');
        }
    }
    if !out_headers.contains("Access-Control-Expose-Headers") {
        out_headers.push_str("Access-Control-Expose-Headers: Tauri-Response, Content-Type\n");
    }

    let body = String::from_utf8_lossy(response.body()).into_owned();
    #[cfg(debug_assertions)]
    eprintln!("[tauri-runtime-qt] {scheme} {method} {url} -> {status} ({} body bytes)", body.len());

    ffi::QtSchemeResponse {
        handled: 1,
        status,
        headers: out_headers,
        body,
    }
}
