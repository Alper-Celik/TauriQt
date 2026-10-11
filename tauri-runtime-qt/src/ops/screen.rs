// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Screen / monitor / cursor operations (owned by the screen worker).
//!
//! This is the provider module other workers consume for monitors: the window
//! and runtime workers must call these functions instead of querying Qt
//! themselves, so the mapping lives in exactly one place.
//!
//! Units: Qt answers in device-independent (logical) global pixels — the space
//! `QScreen::geometry()` and `QGuiApplication::screenAt()` use, and the one
//! tao accepts for `monitor_from_point` on Linux
//! (`gdk_display_get_monitor_at_point`). The values returned here are physical
//! pixels instead: logical lengths are multiplied by the screen's
//! `devicePixelRatio` with the same rounding as `dpi`'s `to_physical`, which is
//! what tao reports for `Monitor` on X11. So the `monitor_from_point` argument is
//! logical while `Monitor::position`/`size` are physical, exactly as in tao.
//! `cursor_position` is physical too (Qt's logical position times the scale
//! factor of the screen under the cursor).

use tauri_runtime::dpi::{PhysicalPosition, PhysicalRect, PhysicalSize};
use tauri_runtime::monitor::Monitor;
use tauri_runtime::{Error, Result};

use crate::bridge::screen::ffi::{self, QtScreenInfo};

/// Logical length → physical pixel (same rounding as `dpi`'s `to_physical`).
fn to_physical(value: i32, scale_factor: f64) -> i32 {
    (f64::from(value) * scale_factor).round() as i32
}

/// Logical length → physical pixel, clamped to the unsigned range.
fn to_physical_u32(value: i32, scale_factor: f64) -> u32 {
    to_physical(value, scale_factor).max(0) as u32
}

/// Turns one Qt screen into a tauri [`Monitor`] (physical pixels).
fn to_monitor(info: &QtScreenInfo) -> Monitor {
    // A missing/zero ratio would silently collapse every value; Qt always
    // reports >= 1, so only an impossible value falls back to 1.
    let scale_factor = if info.scale_factor.is_finite() && info.scale_factor > 0.0 {
        info.scale_factor
    } else {
        1.0
    };

    Monitor {
        name: (!info.name.is_empty()).then(|| info.name.clone()),
        size: PhysicalSize::new(
            to_physical_u32(info.width, scale_factor),
            to_physical_u32(info.height, scale_factor),
        ),
        position: PhysicalPosition::new(
            to_physical(info.x, scale_factor),
            to_physical(info.y, scale_factor),
        ),
        work_area: PhysicalRect {
            position: PhysicalPosition::new(
                to_physical(info.available_x, scale_factor),
                to_physical(info.available_y, scale_factor),
            ),
            size: PhysicalSize::new(
                to_physical_u32(info.available_width, scale_factor),
                to_physical_u32(info.available_height, scale_factor),
            ),
        },
        scale_factor,
    }
}

/// Monitors can only be answered once Qt has a live `QGuiApplication`; before
/// that, reporting an empty list would claim there are no screens.
fn ensure_ready() -> Result<()> {
    if ffi::qt_screen_application_ready() {
        Ok(())
    } else {
        Err(Error::FailedToGetMonitor)
    }
}

/// All monitors, in Qt's screen order.
pub(crate) fn available_monitors() -> Result<Vec<Monitor>> {
    ensure_ready()?;
    let monitors: Vec<Monitor> = ffi::qt_screen_monitors().iter().map(to_monitor).collect();

    #[cfg(debug_assertions)]
    if std::env::var_os("TAURIQT_SCREEN_DEBUG").is_some() {
        eprintln!("[tauri-runtime-qt] available_monitors: {monitors:#?}");
    }

    Ok(monitors)
}

/// The primary monitor, if the platform identifies one.
pub(crate) fn primary_monitor() -> Result<Option<Monitor>> {
    ensure_ready()?;
    let info = ffi::qt_screen_primary();
    Ok(info.valid.then(|| to_monitor(&info)))
}

/// The monitor containing the given desktop point (device-independent global
/// pixels, the space `QGuiApplication::screenAt` uses).
pub(crate) fn monitor_from_point(x: f64, y: f64) -> Result<Option<Monitor>> {
    ensure_ready()?;
    let info = ffi::qt_screen_at_point(x, y);
    Ok(info.valid.then(|| to_monitor(&info)))
}

/// The monitor the given window currently sits on.
pub(crate) fn monitor_for_window(native: u64) -> Result<Option<Monitor>> {
    ensure_ready()?;
    let lookup = ffi::qt_screen_for_window(native);
    if !lookup.window_known {
        return Err(Error::WindowNotFound);
    }
    Ok(lookup.screen.valid.then(|| to_monitor(&lookup.screen)))
}

/// Cursor position relative to the top-left corner of the virtual desktop
/// (physical pixels; Qt reports device-independent ones).
pub(crate) fn cursor_position() -> Result<PhysicalPosition<f64>> {
    let cursor = ffi::qt_screen_cursor_position();
    if !cursor.available {
        // Wayland does not expose a global pointer position (and without a
        // QGuiApplication there is nothing to ask) — say so instead of
        // reporting a stale or invented point.
        return Err(Error::FailedToGetCursorPosition);
    }
    let scale_factor = if cursor.scale_factor.is_finite() && cursor.scale_factor > 0.0 {
        cursor.scale_factor
    } else {
        1.0
    };
    Ok(PhysicalPosition::new(
        cursor.x * scale_factor,
        cursor.y * scale_factor,
    ))
}
