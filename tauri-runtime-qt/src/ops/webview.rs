// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Webview operations (owned by the webview worker).
//!
//! Delegates to `bridge::webview::ffi` (`qt_webview.cpp`).
//!
//! # Units
//!
//! Qt and tauri disagree on pixel units: `QWidget::geometry()` and
//! `QWidget::resize()/move()` are in device-independent (logical) pixels, while
//! tauri's `Physical*` values are device pixels. The conversion uses the
//! widget's `devicePixelRatioF()` (the *window's* dpr — correct under Wayland
//! fractional scaling, where a `QScreen`'s dpr can differ). Getters convert
//! Qt → physical in `qt_webview.cpp`; setters tell the C++ side whether the
//! caller passed logical (`Size::Logical`, used as-is) or physical values
//! (divided by the dpr), so the scale factor is applied in exactly one place.
//!
//! Qt widget geometry is integral *logical* pixels, so it is quantised to the
//! dpr grid: a physical setter followed by a physical getter can differ by up
//! to half a logical pixel (measured at dpr 1.5: `set_position(Physical(5, 7))`
//! reads back as `(5, 8)`). That is Qt's granularity, not an invented value.
//! Positions are relative to the window's client-area origin; sizes and
//! positions of a widget that has not been attached to a window yet are
//! relative to the screen.
//!
//! # Geometry ownership
//!
//! By default the webview is layout-managed: the window agent
//! (`qt_window_attach_webview`) puts the view in the window's zero-margin
//! `QVBoxLayout`, so it fills the client area and follows window resizes.
//! `set_bounds`/`set_size`/`set_position` cannot be expressed through that
//! layout, so the first such call detaches the view from it and Qt stops
//! managing its geometry: afterwards the webview no longer resizes with its
//! window (tauri's `set_auto_resize` is a no-op on this runtime) until it is
//! re-attached with `reparent`. Getters always report the live widget geometry,
//! whoever set it.

use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize, Position, Rect, Size};
use tauri_runtime::{Error, Result};
use tauri_runtime_qt_core as qtcore;
use tauri_utils::config::Color;

use crate::bridge::webview::ffi;

pub(crate) fn url(native: u64) -> Result<String> {
    Ok(ffi::qt_webview_url(native))
}

pub(crate) fn navigate(native: u64, url: &str) -> Result<()> {
    ffi::qt_webview_navigate(native, url);
    Ok(())
}

pub(crate) fn reload(native: u64) -> Result<()> {
    ffi::qt_webview_reload(native);
    Ok(())
}

pub(crate) fn eval(native: u64, script: &str) -> Result<()> {
    ffi::qt_webview_eval(native, script);
    Ok(())
}

pub(crate) fn set_zoom(native: u64, factor: f64) -> Result<()> {
    ffi::qt_webview_set_zoom(native, factor);
    Ok(())
}

pub(crate) fn set_background_color(native: u64, color: Option<Color>) -> Result<()> {
    if let Some(Color(r, g, b, a)) = color {
        ffi::qt_webview_set_background_color(native, r as i32, g as i32, b as i32, a as i32);
    }
    Ok(())
}

/// Places the webview at `bounds` (position + size), the same way
/// `set_position` + `set_size` do; a `Rect` may mix logical and physical
/// components, so each half is converted on its own.
pub(crate) fn set_bounds(native: u64, bounds: Rect) -> Result<()> {
    set_position(native, bounds.position)?;
    set_size(native, bounds.size)
}

pub(crate) fn show(native: u64) -> Result<()> {
    ffi::qt_webview_show(native);
    Ok(())
}

pub(crate) fn hide(native: u64) -> Result<()> {
    ffi::qt_webview_hide(native);
    Ok(())
}

pub(crate) fn close(native: u64) -> Result<()> {
    ffi::qt_webview_close(native);
    // The view (and its page) is going away, so any `runJavaScript` result that
    // is still pending will never arrive: drop its callback instead of leaking
    // it for the lifetime of the process.
    PENDING_EVAL
        .lock()
        .retain(|(_, webview, _)| *webview != native);
    Ok(())
}

pub(crate) fn print(native: u64) -> Result<()> {
    ffi::qt_webview_print(native);
    Ok(())
}

#[cfg(any(debug_assertions, feature = "devtools"))]
pub(crate) fn open_devtools(native: u64) {
    ffi::qt_webview_open_devtools(native);
}

#[cfg(any(debug_assertions, feature = "devtools"))]
pub(crate) fn close_devtools(native: u64) {
    ffi::qt_webview_close_devtools(native);
}

#[cfg(any(debug_assertions, feature = "devtools"))]
pub(crate) fn is_devtools_open(native: u64) -> Result<bool> {
    Ok(ffi::qt_webview_is_devtools_open(native))
}

// ---- geometry ----

pub(crate) fn bounds(native: u64) -> Result<Rect> {
    let geometry = live_geometry(native)?;
    Ok(qtcore::webview::geometry_rect(
        geometry.x,
        geometry.y,
        geometry.width,
        geometry.height,
    ))
}

pub(crate) fn position(native: u64) -> Result<PhysicalPosition<i32>> {
    let geometry = live_geometry(native)?;
    Ok(PhysicalPosition::new(geometry.x, geometry.y))
}

pub(crate) fn size(native: u64) -> Result<PhysicalSize<u32>> {
    let geometry = live_geometry(native)?;
    Ok(qtcore::webview::geometry_size(
        geometry.width,
        geometry.height,
    ))
}

pub(crate) fn set_size(native: u64, size: Size) -> Result<()> {
    let (width, height, physical) = qtcore::webview::size_params(size);
    if ffi::qt_webview_set_size(native, width, height, physical) {
        Ok(())
    } else {
        Err(missing(native))
    }
}

pub(crate) fn set_position(native: u64, position: Position) -> Result<()> {
    // A webview is a child widget, so this positions it inside its own window.
    // That works on Wayland too (unlike positioning a *top-level window*, which
    // the protocol forbids): a child's placement is client-side.
    let (x, y, physical) = qtcore::webview::position_params(position);
    if ffi::qt_webview_set_position(native, x, y, physical) {
        Ok(())
    } else {
        Err(missing(native))
    }
}

pub(crate) fn set_focus(native: u64) -> Result<()> {
    if ffi::qt_webview_set_focus(native) {
        Ok(())
    } else {
        Err(missing(native))
    }
}

// ---- script evaluation with a result callback ----

/// Monotonic id for the pending-eval registry; never reused, so a late result
/// can never be delivered to a newer callback.
static NEXT_EVAL_REQUEST: AtomicU64 = AtomicU64::new(1);

/// `(request id, webview handle, callback)` for `runJavaScript` calls whose
/// result has not arrived yet. `Vec` instead of a map so the whole registry can
/// live in a `const`-initialisable `static`.
static PENDING_EVAL: Mutex<Vec<(u64, u64, Box<dyn Fn(String) + Send + 'static>)>> =
    Mutex::new(Vec::new());

pub(crate) fn eval_with_callback(
    native: u64,
    script: &str,
    callback: impl Fn(String) + Send + 'static,
) -> Result<()> {
    let request_id = NEXT_EVAL_REQUEST.fetch_add(1, Ordering::Relaxed);
    PENDING_EVAL
        .lock()
        .push((request_id, native, Box::new(callback)));

    if ffi::qt_webview_eval_with_callback(native, request_id, script) {
        Ok(())
    } else {
        PENDING_EVAL.lock().retain(|(id, _, _)| *id != request_id);
        Err(missing(native))
    }
}

/// Delivers a `runJavaScript` result (`json`, see the bridge) to the callback
/// registered for `request_id`. Called from Qt on the GUI thread.
///
/// The callback runs outside the registry lock, so a callback that calls back
/// into the webview API cannot deadlock; an unknown id (a result for a webview
/// that was closed, or a duplicate) is dropped.
pub(crate) fn on_eval_result(request_id: u64, json: &str) {
    let callback = {
        let mut pending = PENDING_EVAL.lock();
        match pending.iter().position(|(id, _, _)| *id == request_id) {
            Some(index) => Some(pending.swap_remove(index).2),
            None => None,
        }
    };

    match callback {
        Some(callback) => callback(json.to_owned()),
        None => {
            crate::qtrace!(
                "[tauri-runtime-qt] eval result for unknown/expired request {request_id} ({json})"
            );
        }
    }
}

// ---- reparenting / profile data ----

pub(crate) fn reparent(native: u64, window_native: u64) -> Result<()> {
    if ffi::qt_webview_reparent(native, window_native) {
        Ok(())
    } else {
        Err(Error::CreateWebview(
            format!(
                "tauri-runtime-qt: cannot reparent webview {native} into window {window_native} \
                 (unknown webview or window)"
            )
            .into(),
        ))
    }
}

/// Clears the browsing data Qt 6.11 exposes: HTTP cache, cookies and visited
/// links; see the implementation comment in `qt_webview.cpp` for the stores it
/// cannot touch (localStorage, IndexedDB, service workers) — Qt offers no API
/// for those on a live profile. Fire-and-forget: Qt's cache clearing and cookie
/// deletion are asynchronous.
pub(crate) fn clear_all_browsing_data(native: u64) -> Result<()> {
    if ffi::qt_webview_clear_browsing_data(native) {
        Ok(())
    } else {
        Err(missing(native))
    }
}

// ---- diagnostics ----

/// `TAURIQT_EVAL_DEBUG=1` smoke test for `eval_with_callback`: runs
/// `document.title = 'eval-ok'` and prints whatever the page reports back.
///
/// Triggered from `qt_webview.cpp` when the *first* webview finishes its first
/// page load. Unset (the normal case) this returns immediately, so it costs one
/// `getenv` per process.
pub(crate) fn debug_hook_first_load(native: u64) {
    if std::env::var("TAURIQT_EVAL_DEBUG").ok().as_deref() != Some("1") {
        return;
    }

    let result = eval_with_callback(native, "document.title = 'eval-ok'", |json| {
        eprintln!("[tauri-runtime-qt] TAURIQT_EVAL_DEBUG eval_with_callback -> {json}");
    });
    match result {
        Ok(()) => eprintln!(
            "[tauri-runtime-qt] TAURIQT_EVAL_DEBUG ran document.title = 'eval-ok' on webview {native}"
        ),
        Err(error) => eprintln!("[tauri-runtime-qt] TAURIQT_EVAL_DEBUG failed: {error}"),
    }
}

// ---- helpers ----

/// Live widget geometry in physical pixels, or `Err` for an unknown handle (Qt
/// cannot answer truthfully then, and a made-up `0` would be a lie).
fn live_geometry(native: u64) -> Result<ffi::QtWebviewGeometry> {
    let geometry = ffi::qt_webview_geometry(native);
    if geometry.ok {
        Ok(geometry)
    } else {
        Err(missing(native))
    }
}

/// A handle that is no longer in the Qt registry. Tauri has no dedicated
/// "webview not found" error; `Error::CreateWebview` is this crate's webview
/// error carrier (see `ops`).
fn missing(native: u64) -> Error {
    Error::CreateWebview(format!("tauri-runtime-qt: webview {native} does not exist").into())
}
