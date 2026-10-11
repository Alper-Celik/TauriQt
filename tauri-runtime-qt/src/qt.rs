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
    // in `X-Tauriqt-Payload` (base64); turn it back into the request body here so
    // tauri's IPC handler sees a normal request.
    let mut payload: Option<Vec<u8>> = None;
    let mut request = http::Request::builder().method(method).uri(url);
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(": ") else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.eq_ignore_ascii_case("x-tauriqt-payload") {
            use base64::Engine as _;
            payload = base64::engine::general_purpose::STANDARD.decode(value).ok();
            continue;
        }
        request = request.header(name, value);
    }
    let request_body = payload.unwrap_or_else(|| body.as_bytes().to_vec());
    let Ok(request) = request.body(request_body) else {
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
                let (status, headers, body) = encode_response(response);
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

/// Encodes an `http::Response` into the header/body representation the bridge uses.
fn encode_response(
    response: http::Response<std::borrow::Cow<'static, [u8]>>,
) -> (i32, String, String) {
    let status = response.status().as_u16() as i32;
    let mut headers = String::new();
    for (name, value) in response.headers() {
        if let Ok(value) = value.to_str() {
            headers.push_str(name.as_str());
            headers.push_str(": ");
            headers.push_str(&value.replace(['\r', '\n'], ""));
            headers.push('\n');
        }
    }
    if !headers.contains("Access-Control-Expose-Headers") {
        headers.push_str("Access-Control-Expose-Headers: Tauri-Response, Content-Type\n");
    }
    (status, headers, String::from_utf8_lossy(response.body()).into_owned())
}
