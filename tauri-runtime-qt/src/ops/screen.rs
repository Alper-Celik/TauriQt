// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Screen / monitor / cursor operations (owned by the screen worker).
//!
//! This is the provider module other workers consume for monitors: the window
//! and runtime workers must call these functions instead of querying Qt
//! themselves, so the mapping lives in exactly one place.

#![allow(unused_variables)]

use tauri_runtime::dpi::PhysicalPosition;
use tauri_runtime::monitor::Monitor;
use tauri_runtime::Result;

use super::unimplemented;

/// All monitors, in Qt's screen order.
pub(crate) fn available_monitors() -> Result<Vec<Monitor>> {
    Err(unimplemented("available_monitors"))
}

/// The primary monitor, if the platform identifies one.
pub(crate) fn primary_monitor() -> Result<Option<Monitor>> {
    Err(unimplemented("primary_monitor"))
}

/// The monitor containing the given desktop point.
pub(crate) fn monitor_from_point(x: f64, y: f64) -> Result<Option<Monitor>> {
    Err(unimplemented("monitor_from_point"))
}

/// The monitor the given window currently sits on.
pub(crate) fn monitor_for_window(native: u64) -> Result<Option<Monitor>> {
    Err(unimplemented("current_monitor"))
}

/// Cursor position relative to the top-left corner of the virtual desktop.
pub(crate) fn cursor_position() -> Result<PhysicalPosition<f64>> {
    Err(unimplemented("cursor_position"))
}
