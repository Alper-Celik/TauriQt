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
//!
//! The payload encoding the bridge speaks (header block, base64 body marker,
//! response framing) lives in `tauri-runtime-qt-core::scheme`; the callbacks and
//! the bridge declaration stay here.

use parking_lot::Mutex;
use tauri_runtime as rt;
use tauri_runtime::window::WindowEvent;
use tauri_runtime_qt_core as qtcore;

#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("qt_app.hpp");
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

        // ---- custom schemes (qt_scheme.cpp) ----
        /// Registers ipc/tauri/asset schemes; must run before the QApplication.
        fn qt_scheme_register_schemes();
        /// Installs the scheme handlers into the webview's profile.
        fn qt_scheme_install_handler(webview_id: u64);
        /// Completes a request that `qt_scheme_request` reported as pending
        /// (`handled == 2`).
        fn qt_scheme_respond(request_id: u64, status: i32, headers: &str, body: &str);
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
            request_id: u64,
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
            let (w, h) = qtcore::window::parse_resize_payload(payload);
            WindowEvent::Resized(rt::dpi::PhysicalSize::new(w, h))
        }
        4 => {
            let (x, y) = qtcore::window::parse_move_payload(payload);
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
        crate::qtrace!("[tauri-runtime-qt] webview {id} event {kind}: {payload}");
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
    request_id: u64,
    scheme_kind: i32,
    method: &str,
    url: &str,
    headers: &str,
    body: &str,
) -> ffi::QtSchemeResponse {
        crate::qtrace!("[tauri-runtime-qt] scheme_request webview={webview_id} kind={scheme_kind} {method} {url}");

    let scheme = match scheme_kind {
        0 => "ipc",
        1 => "tauri",
        2 => "asset",
        _ => return unhandled(),
    };

    // QtWebEngine never hands us the POST body, so our invoke system carries it
    // in `X-Tauriqt-Payload` (base64); the core crate turns the bridge's header
    // block back into a normal request for tauri's IPC handler.
    let Some(request) = qtcore::scheme::decode_request(method, url, headers, body) else {
        return unhandled();
    };

    // tauri answers most protocols asynchronously; the responder may fire after
    // this function returned. In that case we hand the response to C++ through
    // `qt_scheme_respond` and report `handled == 2` (pending).
    /// Decides, without a race, whether the responder fired inline or later.
    struct Gate {
        returned: bool,
        immediate: Option<ffi::QtSchemeResponse>,
    }
    let gate = std::sync::Arc::new(Mutex::new(Gate {
        returned: false,
        immediate: None,
    }));
    // The responder may outlive this call, so it must own its diagnostic text.
    let method_owned = method.to_owned();
    let url_owned = url.to_owned();

    {
        let guard = crate::SCHEME_PROTOCOLS.lock();
        let Some(entry) = guard.iter().find(|(id, _, _)| *id == webview_id) else {
            return unhandled();
        };
        let Some(handler) = entry.2.get(scheme) else {
            return unhandled();
        };
        let label = entry.1.clone();

        let gate = std::sync::Arc::clone(&gate);
        let method_owned = method_owned.clone();
        let url_owned = url_owned.clone();
        handler(
            &label,
            request,
            Box::new(move |response: http::Response<std::borrow::Cow<'static, [u8]>>| {
                let (status, headers, body) = qtcore::scheme::encode_response(response);
                                crate::qtrace!(
                    "[tauri-runtime-qt] {scheme} {method_owned} {url_owned} -> {status} ({} body bytes)",
                    body.len()
                );
                // If `qt_scheme_request` has not returned yet the response can
                // travel back inline; otherwise it must be delivered through
                // `qt_scheme_respond`, because this call outlived the bridge call.
                let deferred = {
                    let mut gate = gate.lock();
                    if gate.returned {
                        true
                    } else {
                        gate.immediate = Some(ffi::QtSchemeResponse {
                            handled: 1,
                            status,
                            headers: headers.clone(),
                            body: body.clone(),
                        });
                        false
                    }
                };
                if deferred {
                    ffi::qt_scheme_respond(request_id, status, &headers, &body);
                }
            }),
        );
    }

    // Mark the bridge call as finished while holding the lock, so a responder
    // that fires concurrently either lands in `immediate` or takes the
    // `qt_scheme_respond` path — never neither.
    let inline = {
        let mut gate = gate.lock();
        gate.returned = true;
        gate.immediate.take()
    };
    if let Some(response) = inline {
        return response;
    }

    // Not answered yet: the C++ side keeps the job alive and waits for
    // `qt_scheme_respond`.
    ffi::QtSchemeResponse {
        handled: 2,
        status: 0,
        headers: String::new(),
        body: String::new(),
    }
}
