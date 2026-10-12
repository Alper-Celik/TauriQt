// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Window operations (owned by the window worker).
//!
//! Everything here talks to `bridge::window::ffi` (`qt_window.cpp`). Functions
//! return `Err` when Qt cannot answer truthfully on this platform: an id that
//! is not in the C++ registry is `Error::WindowNotFound`, and an operation the
//! platform cannot honour (an unreportable colour scheme, a refused drag) is a
//! message-carrying error instead of a fabricated value.
//!
//! Window *positions* are deliberately not in that set. A Wayland compositor
//! owns placement and honours neither `move()` nor a stacking hint, but Qt keeps
//! the position an app asked for and reports it back, and callers treat the
//! position as always available: `tauri-plugin-window-state`, for one, aborts
//! its startup restore — including the `show()` that maps a hidden window —
//! on the first `Err` it gets from `outer_position`. So `inner_position`,
//! `outer_position` and `set_position` answer with Qt's own bookkeeping on every
//! platform, exactly like tao does, and report the app's *requested* position on
//! Wayland rather than failing. See `docs/wayland-ci.md`.
//!
//! Qt reports geometry in device-independent ("logical") pixels; tauri asks
//! for physical ones, so every geometry value crosses the boundary through the
//! window's own device pixel ratio (`qt_window_scale_factor`). The conversions
//! themselves (and Qt's 24-bit hint sentinels) live in
//! `tauri-runtime-qt-core::window`.

use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize, Position, Size};
use tauri_runtime::window::WindowSizeConstraints;
use tauri_runtime::{Error, Icon, Result};
use tauri_runtime_qt_core as qtcore;
use tauri_utils::config::Color;
use tauri_utils::Theme;

use crate::bridge::window::ffi;

// ---- helpers ----

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

/// The window's device pixel ratio (physical pixels per logical pixel), with
/// Qt's "not on a screen yet" reading normalised to 1.
fn device_pixel_ratio(native: u64) -> f64 {
    qtcore::window::device_pixel_ratio(ffi::qt_window_scale_factor(native))
}

/// Qt logical pixels → tauri physical pixels.
fn physical_size(native: u64, size: ffi::QtWindowSize) -> PhysicalSize<u32> {
    qtcore::window::to_physical_size(device_pixel_ratio(native), size.width, size.height)
}

/// Qt logical pixels → tauri physical pixels.
fn physical_position(native: u64, point: ffi::QtWindowPoint) -> PhysicalPosition<i32> {
    qtcore::window::to_physical_position(device_pixel_ratio(native), point.x, point.y)
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

/// Position of the window's client area.
///
/// On Wayland this is Qt's own record of where the window was asked to be (the
/// compositor decides where it really is and does not report it), which is what
/// tao answers there too — see the module docs.
pub(crate) fn inner_position(native: u64) -> Result<PhysicalPosition<i32>> {
    require_window(native)?;
    Ok(physical_position(
        native,
        ffi::qt_window_inner_position(native),
    ))
}

/// Position of the window's frame (its top-left corner on the desktop).
///
/// Same Wayland caveat as [`inner_position`]: the value is the requested
/// position, not a compositor-reported one.
pub(crate) fn outer_position(native: u64) -> Result<PhysicalPosition<i32>> {
    require_window(native)?;
    Ok(physical_position(
        native,
        ffi::qt_window_outer_position(native),
    ))
}

pub(crate) fn title(native: u64) -> Result<String> {
    require_window(native)?;
    Ok(ffi::qt_window_title(native))
}

pub(crate) fn theme(native: u64) -> Result<Theme> {
    require_window(native)?;
    let code = ffi::qt_window_theme(native);
    qtcore::window::theme_from_code(code).ok_or_else(|| {
        unavailable(
            "the window theme",
            &format!("Qt reports the colour scheme as unknown (code {code})"),
        )
    })
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
        Some(size) => qtcore::window::qt_size(device_pixel_ratio(native), size),
        // Qt's "no minimum size": QSize(0, 0).
        None => qtcore::window::NO_MINIMUM,
    };
    ffi::qt_window_set_min_size(native, width, height);
    Ok(())
}

pub(crate) fn set_max_size(native: u64, size: Option<Size>) -> Result<()> {
    require_window(native)?;
    let (width, height) = match size {
        Some(size) => qtcore::window::qt_size(device_pixel_ratio(native), size),
        // Qt's "no maximum size": QWIDGETSIZE_MAX.
        None => qtcore::window::NO_MAXIMUM,
    };
    ffi::qt_window_set_max_size(native, width, height);
    Ok(())
}

pub(crate) fn set_size_constraints(native: u64, constraints: WindowSizeConstraints) -> Result<()> {
    require_window(native)?;
    let hints = qtcore::window::size_hints(device_pixel_ratio(native), constraints);
    // Both hints are written in one GUI-thread turn, so no resize can observe
    // half of the new constraints.
    ffi::qt_window_set_min_size(native, hints.min_width, hints.min_height);
    ffi::qt_window_set_max_size(native, hints.max_width, hints.max_height);
    Ok(())
}

pub(crate) fn set_icon(native: u64, icon: Icon) -> Result<()> {
    require_window(native)?;
    if let Some(message) =
        qtcore::window::icon_buffer_error(icon.width, icon.height, icon.rgba.len())
    {
        return Err(Error::InvalidIcon(message.into()));
    }
    if !ffi::qt_window_set_icon(native, icon.rgba.as_ref(), icon.width, icon.height) {
        return Err(Error::InvalidIcon("Qt rejected the icon pixels".into()));
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
    if !qtcore::window::placement_supported(&platform) {
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
    if !qtcore::window::placement_supported(&platform) {
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

/// Moves the window to `position`, in the window's own pixel unit.
///
/// On X11 the window manager is asked to place the frame there. On Wayland the
/// compositor owns placement, so `move()` only records the position Qt reports
/// back through [`outer_position`]; the call still succeeds, as it does in tao.
pub(crate) fn set_position(native: u64, position: Position) -> Result<()> {
    require_window(native)?;
    let (x, y) = qtcore::window::window_position(device_pixel_ratio(native), position);
    ffi::qt_window_set_position(native, x, y);
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
