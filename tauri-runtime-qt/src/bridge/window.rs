// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Window declarations (owned by the window worker).
//!
//! C++ translation units include the cxx-generated header for this bridge:
//! `#include "tauri-runtime-qt/src/bridge/window.rs.h"`.

#[cxx::bridge]
pub mod ffi {
    /// A size in Qt device-independent ("logical") pixels.
    struct QtWindowSize {
        width: f64,
        height: f64,
    }

    /// A point in Qt device-independent ("logical") pixels.
    struct QtWindowPoint {
        x: f64,
        y: f64,
    }

    unsafe extern "C++" {
        include!("qt_window.hpp");

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

        // ---- window worker: registry and platform ----
        /// Whether `id` names a live window in the C++ registry. Rust answers
        /// `Error::WindowNotFound` when it does not, instead of guessing.
        fn qt_window_alive(id: u64) -> bool;

        /// Name of the Qt platform plugin in use (e.g. `"xcb"`, `"wayland"`).
        fn qt_window_platform() -> String;

        // ---- geometry (Qt logical pixels; Rust applies the scale factor) ----
        /// Client area size (`QWidget::size()`).
        fn qt_window_inner_size(id: u64) -> QtWindowSize;
        /// Frame size including window manager decorations
        /// (`QWidget::frameGeometry()`).
        fn qt_window_outer_size(id: u64) -> QtWindowSize;
        /// Client area origin on the desktop (`QWidget::mapToGlobal(0, 0)`).
        fn qt_window_inner_position(id: u64) -> QtWindowPoint;
        /// Frame origin on the desktop (`QWidget::frameGeometry().topLeft()`).
        fn qt_window_outer_position(id: u64) -> QtWindowPoint;

        // ---- state ----
        fn qt_window_title(id: u64) -> String;
        /// `QStyleHints::colorScheme()`: 1 = light, 2 = dark, 0 = unknown.
        fn qt_window_theme(id: u64) -> i32;
        fn qt_window_is_fullscreen(id: u64) -> bool;
        fn qt_window_is_minimized(id: u64) -> bool;
        fn qt_window_is_maximized(id: u64) -> bool;
        fn qt_window_is_focused(id: u64) -> bool;
        fn qt_window_is_decorated(id: u64) -> bool;
        fn qt_window_is_resizable(id: u64) -> bool;
        fn qt_window_is_enabled(id: u64) -> bool;
        fn qt_window_is_always_on_top(id: u64) -> bool;
        /// Qt's `WindowMaximizeButtonHint` / `WindowMinimizeButtonHint` /
        /// `WindowCloseButtonHint` bits (the hints handed to the window
        /// manager; Qt offers no per-button state beyond them).
        fn qt_window_is_maximizable(id: u64) -> bool;
        fn qt_window_is_minimizable(id: u64) -> bool;
        fn qt_window_is_closable(id: u64) -> bool;

        // ---- setters ----
        fn qt_window_set_fullscreen(id: u64, fullscreen: bool);
        fn qt_window_minimize(id: u64);
        fn qt_window_unminimize(id: u64);
        /// Qt logical pixels; `0` / `QWIDGETSIZE_MAX` mean "no min/max".
        fn qt_window_set_min_size(id: u64, width: f64, height: f64);
        fn qt_window_set_max_size(id: u64, width: f64, height: f64);
        /// Takes the icon's straight RGBA8888 bytes. Returns false when the
        /// buffer does not match `width * height * 4` (Rust pre-validates too).
        fn qt_window_set_icon(id: u64, rgba: &[u8], width: u32, height: u32) -> bool;
        fn qt_window_set_focus(id: u64);
        fn qt_window_set_resizable(id: u64, resizable: bool);
        fn qt_window_set_enabled(id: u64, enabled: bool);
        fn qt_window_set_always_on_top(id: u64, on_top: bool);
        fn qt_window_set_always_on_bottom(id: u64, on_bottom: bool);
        fn qt_window_set_background_color(id: u64, r: u8, g: u8, b: u8, a: u8);
        /// Restores the application palette (tauri's `None` background).
        fn qt_window_reset_background_color(id: u64);
        /// Moves the frame origin (X11 only; Rust rejects other platforms).
        fn qt_window_set_position(id: u64, x: i32, y: i32);
        /// Returns false when Qt cannot start the system move (no native
        /// window handle, or not called from a mouse press/release handler).
        fn qt_window_start_dragging(id: u64) -> bool;
        /// `direction` is tauri's `ResizeDirection` number (1..8, as mapped by
        /// `lib.rs`); Qt maps it to `Qt::Edges`. Returns false when Qt cannot
        /// start the system resize or the direction is unknown.
        fn qt_window_start_resize_dragging(id: u64, direction: i32) -> bool;
    }
}
