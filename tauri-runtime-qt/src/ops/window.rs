// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Window operations (owned by the window worker).
//!
//! Everything here talks to `bridge::window::ffi` (`qt_window.cpp`). Functions
//! return `Err` when Qt cannot answer truthfully on this platform: an id that
//! is not in the C++ registry is `Error::WindowNotFound`, and an operation the
//! platform cannot honour (Wayland placement, an unreportable colour scheme,
//! a refused drag) is a message-carrying error instead of a fabricated value.
//!
//! Qt reports geometry in device-independent ("logical") pixels; tauri asks
//! for physical ones, so every geometry value crosses the boundary through the
//! window's own device pixel ratio (`qt_window_scale_factor`).

use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize, PixelUnit, Position, Size};
use tauri_runtime::window::WindowSizeConstraints;
use tauri_runtime::{Error, Icon, Result};
use tauri_utils::config::Color;
use tauri_utils::Theme;

use crate::bridge::window::ffi;

// ---- helpers ----

/// Qt's largest usable window dimension (`QWIDGETSIZE_MAX`): the sentinel of
/// "no maximum size".
const QWIDGETSIZE_MAX: f64 = (1 << 24) as f64 - 1.0;

/// Fails with `Error::WindowNotFound` for an id that is not in the C++ window
/// registry, instead of asking Qt about a window that is not there.
fn require_window(native: u64) -> Result<()> {
    if ffi::qt_window_alive(native) {
        Ok(())
    } else {
        Err(Error::WindowNotFound)
    }
}

/// A truthful error for an operation Qt cannot honour here.
///
/// `tauri_runtime::Error` has no dedicated "unsupported" variant, so the
/// message-carrying variant is used (as `ops::unimplemented` does for the
/// operations that do not exist at all).
fn unavailable(what: &str, why: &str) -> Error {
    Error::CreateWebview(format!("tauri-runtime-qt: {what} is unavailable: {why}").into())
}

/// Name of the Qt platform plugin in use (e.g. `"xcb"`, `"wayland"`).
fn platform() -> String {
    ffi::qt_window_platform()
}

/// Whether the platform plugin lets clients place and stack their own
/// top-level windows (X11). A Wayland compositor owns placement: the protocol
/// has no request for it and Qt answers position queries with artificial
/// values, so Rust must refuse instead of reporting those.
fn placement_supported(platform: &str) -> bool {
    matches!(platform, "xcb" | "xlib")
}

/// The window's device pixel ratio (physical pixels per logical pixel).
fn device_pixel_ratio(native: u64) -> f64 {
    let dpr = ffi::qt_window_scale_factor(native);
    if dpr > 0.0 {
        dpr
    } else {
        1.0
    }
}

/// Qt logical pixels → tauri physical pixels.
fn physical_size(native: u64, size: ffi::QtWindowSize) -> PhysicalSize<u32> {
    let dpr = device_pixel_ratio(native);
    PhysicalSize::new(
        (size.width * dpr).round() as u32,
        (size.height * dpr).round() as u32,
    )
}

/// Qt logical pixels → tauri physical pixels.
fn physical_position(native: u64, point: ffi::QtWindowPoint) -> PhysicalPosition<i32> {
    let dpr = device_pixel_ratio(native);
    PhysicalPosition::new(
        (point.x * dpr).round() as i32,
        (point.y * dpr).round() as i32,
    )
}

/// Qt size hints are 24-bit: clamp so no `f64` can overflow the `int` Qt
/// takes. A non-finite value (e.g. the `PixelUnit::MAX` sentinel) means "no
/// constraint", i.e. the maximum.
fn qt_dimension(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, QWIDGETSIZE_MAX)
    }
}

/// A tauri size in Qt logical pixels.
fn qt_size(dpr: f64, size: Size) -> (f64, f64) {
    let (width, height) = match size {
        Size::Logical(size) => (size.width, size.height),
        Size::Physical(size) => (f64::from(size.width) / dpr, f64::from(size.height) / dpr),
    };
    (qt_dimension(width), qt_dimension(height))
}

/// A tauri size-constraint value in Qt logical pixels (`missing` when the
/// constraint is unspecified).
fn qt_pixel_unit(dpr: f64, unit: Option<PixelUnit>, missing: f64) -> f64 {
    match unit {
        None => missing,
        Some(PixelUnit::Logical(unit)) => qt_dimension(unit.0),
        Some(PixelUnit::Physical(unit)) => qt_dimension(f64::from(unit.0) / dpr),
    }
}

// ---- lifecycle ----

pub(crate) fn show(native: u64) -> Result<()> {
    ffi::qt_window_show(native);
    Ok(())
}

pub(crate) fn hide(native: u64) -> Result<()> {
    ffi::qt_window_hide(native);
    Ok(())
}

pub(crate) fn close(native: u64) -> Result<()> {
    ffi::qt_window_close(native);
    Ok(())
}

pub(crate) fn destroy(native: u64) -> Result<()> {
    ffi::qt_window_close(native);
    Ok(())
}

// ---- getters ----

pub(crate) fn scale_factor(native: u64) -> Result<f64> {
    Ok(ffi::qt_window_scale_factor(native))
}

pub(crate) fn inner_size(native: u64) -> Result<PhysicalSize<u32>> {
    require_window(native)?;
    Ok(physical_size(native, ffi::qt_window_inner_size(native)))
}

pub(crate) fn outer_size(native: u64) -> Result<PhysicalSize<u32>> {
    require_window(native)?;
    let size = ffi::qt_window_outer_size(native);
    if size.width <= 0.0 || size.height <= 0.0 {
        // An empty frame rectangle means the platform does not report the
        // frame; a `0x0` outer size would be a fabricated value.
        return Err(unavailable(
            "the window frame size",
            "the platform does not report the window frame geometry",
        ));
    }
    Ok(physical_size(native, size))
}

pub(crate) fn inner_position(native: u64) -> Result<PhysicalPosition<i32>> {
    require_window(native)?;
    let platform = platform();
    if !placement_supported(&platform) {
        return Err(unavailable(
            "the window position",
            &format!("the '{platform}' Qt platform does not report top-level window positions"),
        ));
    }
    Ok(physical_position(native, ffi::qt_window_inner_position(native)))
}

pub(crate) fn outer_position(native: u64) -> Result<PhysicalPosition<i32>> {
    require_window(native)?;
    let platform = platform();
    if !placement_supported(&platform) {
        return Err(unavailable(
            "the window frame position",
            &format!("the '{platform}' Qt platform does not report top-level window positions"),
        ));
    }
    Ok(physical_position(native, ffi::qt_window_outer_position(native)))
}

pub(crate) fn title(native: u64) -> Result<String> {
    require_window(native)?;
    Ok(ffi::qt_window_title(native))
}

pub(crate) fn theme(native: u64) -> Result<Theme> {
    require_window(native)?;
    match ffi::qt_window_theme(native) {
        1 => Ok(Theme::Light),
        2 => Ok(Theme::Dark),
        code => Err(unavailable(
            "the window theme",
            &format!("Qt reports the colour scheme as unknown (code {code})"),
        )),
    }
}

pub(crate) fn is_fullscreen(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_fullscreen(native))
}

pub(crate) fn is_minimized(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_minimized(native))
}

pub(crate) fn is_maximized(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_maximized(native))
}

pub(crate) fn is_focused(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_focused(native))
}

pub(crate) fn is_decorated(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_decorated(native))
}

pub(crate) fn is_resizable(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_resizable(native))
}

pub(crate) fn is_visible(native: u64) -> Result<bool> {
    Ok(ffi::qt_window_is_visible(native))
}

pub(crate) fn is_enabled(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_enabled(native))
}

pub(crate) fn is_always_on_top(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_always_on_top(native))
}

pub(crate) fn is_maximizable(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_maximizable(native))
}

pub(crate) fn is_minimizable(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_minimizable(native))
}

pub(crate) fn is_closable(native: u64) -> Result<bool> {
    require_window(native)?;
    Ok(ffi::qt_window_is_closable(native))
}

// ---- setters ----

pub(crate) fn set_title(native: u64, title: &str) -> Result<()> {
    ffi::qt_window_set_title(native, title);
    Ok(())
}

pub(crate) fn set_size(native: u64, width: i32, height: i32) -> Result<()> {
    ffi::qt_window_set_size(native, width, height);
    Ok(())
}

pub(crate) fn center(native: u64) -> Result<()> {
    ffi::qt_window_center(native);
    Ok(())
}

pub(crate) fn set_decorations(native: u64, decorations: bool) -> Result<()> {
    ffi::qt_window_set_decorations(native, decorations);
    Ok(())
}

pub(crate) fn set_maximized(native: u64, maximized: bool) -> Result<()> {
    ffi::qt_window_set_maximized(native, maximized);
    Ok(())
}

pub(crate) fn minimize(native: u64) -> Result<()> {
    require_window(native)?;
    ffi::qt_window_minimize(native);
    Ok(())
}

pub(crate) fn unminimize(native: u64) -> Result<()> {
    require_window(native)?;
    ffi::qt_window_unminimize(native);
    Ok(())
}

pub(crate) fn set_fullscreen(native: u64, fullscreen: bool) -> Result<()> {
    require_window(native)?;
    ffi::qt_window_set_fullscreen(native, fullscreen);
    Ok(())
}

pub(crate) fn set_min_size(native: u64, size: Option<Size>) -> Result<()> {
    require_window(native)?;
    let (width, height) = match size {
        Some(size) => qt_size(device_pixel_ratio(native), size),
        // Qt's "no minimum size": QSize(0, 0).
        None => (0.0, 0.0),
    };
    ffi::qt_window_set_min_size(native, width, height);
    Ok(())
}

pub(crate) fn set_max_size(native: u64, size: Option<Size>) -> Result<()> {
    require_window(native)?;
    let (width, height) = match size {
        Some(size) => qt_size(device_pixel_ratio(native), size),
        // Qt's "no maximum size": QWIDGETSIZE_MAX.
        None => (QWIDGETSIZE_MAX, QWIDGETSIZE_MAX),
    };
    ffi::qt_window_set_max_size(native, width, height);
    Ok(())
}

pub(crate) fn set_size_constraints(native: u64, constraints: WindowSizeConstraints) -> Result<()> {
    require_window(native)?;
    let dpr = device_pixel_ratio(native);
    let min_width = qt_pixel_unit(dpr, constraints.min_width, 0.0);
    let min_height = qt_pixel_unit(dpr, constraints.min_height, 0.0);
    let max_width = qt_pixel_unit(dpr, constraints.max_width, QWIDGETSIZE_MAX);
    let max_height = qt_pixel_unit(dpr, constraints.max_height, QWIDGETSIZE_MAX);
    // Both hints are written in one GUI-thread turn, so no resize can observe
    // half of the new constraints.
    ffi::qt_window_set_min_size(native, min_width, min_height);
    ffi::qt_window_set_max_size(native, max_width, max_height);
    Ok(())
}

pub(crate) fn set_icon(native: u64, icon: Icon) -> Result<()> {
    require_window(native)?;
    let expected = (icon.width as usize)
        .saturating_mul(icon.height as usize)
        .saturating_mul(4);
    if icon.width == 0 || icon.height == 0 || icon.rgba.len() != expected {
        return Err(Error::InvalidIcon(
            format!(
                "icon is {}x{} RGBA8888 ({} bytes) but the buffer holds {} bytes",
                icon.width,
                icon.height,
                expected,
                icon.rgba.len()
            )
            .into(),
        ));
    }
    if !ffi::qt_window_set_icon(native, icon.rgba.as_ref(), icon.width, icon.height) {
        return Err(Error::InvalidIcon(
            "Qt rejected the icon pixels".into(),
        ));
    }
    Ok(())
}

pub(crate) fn set_focus(native: u64) -> Result<()> {
    require_window(native)?;
    ffi::qt_window_set_focus(native);
    Ok(())
}

pub(crate) fn set_resizable(native: u64, resizable: bool) -> Result<()> {
    require_window(native)?;
    ffi::qt_window_set_resizable(native, resizable);
    Ok(())
}

pub(crate) fn set_enabled(native: u64, enabled: bool) -> Result<()> {
    require_window(native)?;
    ffi::qt_window_set_enabled(native, enabled);
    Ok(())
}

pub(crate) fn set_always_on_bottom(native: u64, always_on_bottom: bool) -> Result<()> {
    require_window(native)?;
    let platform = platform();
    if !placement_supported(&platform) {
        return Err(unavailable(
            "always-on-bottom",
            &format!(
                "the '{platform}' Qt platform has no on-bottom state (a Wayland compositor does not expose one)"
            ),
        ));
    }
    ffi::qt_window_set_always_on_bottom(native, always_on_bottom);
    Ok(())
}

pub(crate) fn set_always_on_top(native: u64, always_on_top: bool) -> Result<()> {
    require_window(native)?;
    let platform = platform();
    if !placement_supported(&platform) {
        return Err(unavailable(
            "always-on-top",
            &format!(
                "the '{platform}' Qt platform has no on-top state (Wayland compositors ignore Qt::WindowStaysOnTopHint, QTBUG-73456)"
            ),
        ));
    }
    ffi::qt_window_set_always_on_top(native, always_on_top);
    Ok(())
}

pub(crate) fn set_background_color(native: u64, color: Option<Color>) -> Result<()> {
    require_window(native)?;
    match color {
        Some(Color(r, g, b, a)) => ffi::qt_window_set_background_color(native, r, g, b, a),
        None => ffi::qt_window_reset_background_color(native),
    }
    Ok(())
}

pub(crate) fn set_position(native: u64, position: Position) -> Result<()> {
    require_window(native)?;
    let platform = platform();
    if !placement_supported(&platform) {
        return Err(unavailable(
            "absolute window positioning",
            &format!("the '{platform}' Qt platform does not let clients place top-level windows"),
        ));
    }
    let dpr = device_pixel_ratio(native);
    let (x, y) = match position {
        Position::Logical(position) => (position.x, position.y),
        Position::Physical(position) => (f64::from(position.x) / dpr, f64::from(position.y) / dpr),
    };
    ffi::qt_window_set_position(native, x.round() as i32, y.round() as i32);
    Ok(())
}

pub(crate) fn start_dragging(native: u64) -> Result<()> {
    require_window(native)?;
    if ffi::qt_window_start_dragging(native) {
        return Ok(());
    }
    Err(unavailable(
        "window dragging",
        "QWindow::startSystemMove() returned false (no native window handle, or the call did not come from a mouse press/release handler)",
    ))
}

pub(crate) fn start_resize_dragging(native: u64, direction: i32) -> Result<()> {
    require_window(native)?;
    if !(1..=8).contains(&direction) {
        return Err(unavailable(
            "window resize dragging",
            &format!("unknown resize direction {direction}"),
        ));
    }
    if ffi::qt_window_start_resize_dragging(native, direction) {
        return Ok(());
    }
    Err(unavailable(
        "window resize dragging",
        "QWindow::startSystemResize() returned false (no native window handle, or the call did not come from a mouse press/release handler)",
    ))
}
