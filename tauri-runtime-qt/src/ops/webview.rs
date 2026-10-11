// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Webview operations (owned by the webview worker).
//!
//! Delegates to `bridge::webview::ffi` (`qt_webview.cpp`).

#![allow(unused_variables)]

use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize, Position, Rect, Size};
use tauri_runtime::{Error, Result};
use tauri_utils::config::Color;

use super::unimplemented;
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

pub(crate) fn set_bounds(native: u64, bounds: Rect) -> Result<()> {
    let (x, y) = match bounds.position {
        Position::Physical(p) => (p.x, p.y),
        Position::Logical(p) => (p.x as i32, p.y as i32),
    };
    let (w, h) = match bounds.size {
        Size::Physical(s) => (s.width as i32, s.height as i32),
        Size::Logical(s) => (s.width as i32, s.height as i32),
    };
    ffi::qt_webview_set_bounds(native, x, y, w, h);
    Ok(())
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

// ---- to implement (webview worker) ----

/// Geometry is layout-managed by Qt today; the worker should read the real
/// widget geometry instead of the origin.
pub(crate) fn bounds(native: u64) -> Result<Rect> {
    Err(unimplemented("WebviewDispatch::bounds"))
}

pub(crate) fn position(native: u64) -> Result<PhysicalPosition<i32>> {
    Err(unimplemented("WebviewDispatch::position"))
}

pub(crate) fn size(native: u64) -> Result<PhysicalSize<u32>> {
    Err(unimplemented("WebviewDispatch::size"))
}

pub(crate) fn set_size(native: u64, size: Size) -> Result<()> {
    Err(unimplemented("WebviewDispatch::set_size"))
}

pub(crate) fn set_position(native: u64, position: Position) -> Result<()> {
    Err(unimplemented("WebviewDispatch::set_position"))
}

pub(crate) fn set_focus(native: u64) -> Result<()> {
    Err(unimplemented("WebviewDispatch::set_focus"))
}

/// Needs a Rust callback bridging `QWebEnginePage::runJavaScript`'s result.
pub(crate) fn eval_with_callback(
    native: u64,
    script: &str,
    callback: impl Fn(String) + Send + 'static,
) -> Result<()> {
    Err(unimplemented("WebviewDispatch::eval_script_with_callback"))
}

pub(crate) fn reparent(native: u64, window_native: u64) -> Result<()> {
    Err(unimplemented("WebviewDispatch::reparent"))
}

/// Qt offers no complete purge; see PLAN-WORKERS.md "W4 design note".
pub(crate) fn clear_all_browsing_data(native: u64) -> Result<()> {
    Err(unimplemented("WebviewDispatch::clear_all_browsing_data"))
}
