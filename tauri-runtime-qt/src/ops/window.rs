// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Window operations (owned by the window worker).
//!
//! Everything here talks to `bridge::window::ffi` (`qt_window.cpp`). Functions
//! return `Err` when Qt cannot answer truthfully on this platform.

#![allow(unused_variables)]

use tauri_runtime::dpi::{PhysicalPosition, PhysicalSize};
use tauri_runtime::window::WindowSizeConstraints;
use tauri_utils::Theme;
use tauri_runtime::{Error, Icon, Result};

use super::unimplemented;
use crate::bridge::window::ffi;

// ---- implemented today ----

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

pub(crate) fn scale_factor(native: u64) -> Result<f64> {
    Ok(ffi::qt_window_scale_factor(native))
}

pub(crate) fn is_visible(native: u64) -> Result<bool> {
    Ok(ffi::qt_window_is_visible(native))
}

// ---- to implement (window worker) ----

pub(crate) fn inner_size(native: u64) -> Result<PhysicalSize<u32>> {
    Err(unimplemented("WindowDispatch::inner_size"))
}
pub(crate) fn outer_size(native: u64) -> Result<PhysicalSize<u32>> {
    Err(unimplemented("WindowDispatch::outer_size"))
}
pub(crate) fn inner_position(native: u64) -> Result<PhysicalPosition<i32>> {
    Err(unimplemented("WindowDispatch::inner_position"))
}
pub(crate) fn outer_position(native: u64) -> Result<PhysicalPosition<i32>> {
    Err(unimplemented("WindowDispatch::outer_position"))
}
pub(crate) fn title(native: u64) -> Result<String> {
    Err(unimplemented("WindowDispatch::title"))
}
pub(crate) fn theme(native: u64) -> Result<Theme> {
    Err(unimplemented("WindowDispatch::theme"))
}
pub(crate) fn is_fullscreen(native: u64) -> Result<bool> {
    Err(unimplemented("WindowDispatch::is_fullscreen"))
}
pub(crate) fn is_minimized(native: u64) -> Result<bool> {
    Err(unimplemented("WindowDispatch::is_minimized"))
}
pub(crate) fn is_maximized(native: u64) -> Result<bool> {
    Err(unimplemented("WindowDispatch::is_maximized"))
}
pub(crate) fn is_focused(native: u64) -> Result<bool> {
    Err(unimplemented("WindowDispatch::is_focused"))
}
pub(crate) fn is_decorated(native: u64) -> Result<bool> {
    Err(unimplemented("WindowDispatch::is_decorated"))
}
pub(crate) fn is_resizable(native: u64) -> Result<bool> {
    Err(unimplemented("WindowDispatch::is_resizable"))
}
pub(crate) fn is_enabled(native: u64) -> Result<bool> {
    Err(unimplemented("WindowDispatch::is_enabled"))
}
pub(crate) fn is_always_on_top(native: u64) -> Result<bool> {
    Err(unimplemented("WindowDispatch::is_always_on_top"))
}
pub(crate) fn set_fullscreen(native: u64, fullscreen: bool) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_fullscreen"))
}
pub(crate) fn minimize(native: u64) -> Result<()> {
    Err(unimplemented("WindowDispatch::minimize"))
}
pub(crate) fn unminimize(native: u64) -> Result<()> {
    Err(unimplemented("WindowDispatch::unminimize"))
}
pub(crate) fn set_min_size(native: u64, size: Option<tauri_runtime::dpi::Size>) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_min_size"))
}
pub(crate) fn set_max_size(native: u64, size: Option<tauri_runtime::dpi::Size>) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_max_size"))
}
pub(crate) fn set_size_constraints(
    native: u64,
    constraints: WindowSizeConstraints,
) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_size_constraints"))
}
pub(crate) fn set_icon(native: u64, icon: Icon) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_icon"))
}
pub(crate) fn set_focus(native: u64) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_focus"))
}
pub(crate) fn set_resizable(native: u64, resizable: bool) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_resizable"))
}
pub(crate) fn set_enabled(native: u64, enabled: bool) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_enabled"))
}
pub(crate) fn start_dragging(native: u64) -> Result<()> {
    Err(unimplemented("WindowDispatch::start_dragging"))
}
pub(crate) fn start_resize_dragging(native: u64, direction: i32) -> Result<()> {
    Err(unimplemented("WindowDispatch::start_resize_dragging"))
}

pub(crate) fn set_always_on_bottom(native: u64, always_on_bottom: bool) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_always_on_bottom"))
}
pub(crate) fn set_always_on_top(native: u64, always_on_top: bool) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_always_on_top"))
}
pub(crate) fn set_background_color(
    native: u64,
    color: Option<tauri_utils::config::Color>,
) -> Result<()> {
    Err(unimplemented("WindowDispatch::set_background_color"))
}
pub(crate) fn set_position(native: u64, position: tauri_runtime::dpi::Position) -> Result<()> {
    // Wayland cannot position windows absolutely; the worker should return Err
    // there and use QWindow::setPosition on X11.
    Err(unimplemented("WindowDispatch::set_position"))
}
