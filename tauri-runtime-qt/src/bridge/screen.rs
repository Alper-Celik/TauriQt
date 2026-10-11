// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Screen/monitor/cursor declarations (owned by the screen worker).
//!
//! Implemented in `qt_screen.cpp`; C++ translation units include the
//! cxx-generated header for this bridge:
//! `#include "tauri-runtime-qt/src/bridge/screen.rs.h"`.
//!
//! Every geometry value that crosses this bridge is in Qt's device-independent
//! (logical) global pixels, exactly as `QScreen::geometry()` and
//! `QGuiApplication::screenAt()` express them; the `scale_factor` converts to
//! physical pixels and is applied on the Rust side.

#[cxx::bridge]
pub mod ffi {
    /// One `QScreen`.
    struct QtScreenInfo {
        /// false for the placeholder value the lookups return when no screen
        /// matches; the other fields are then zeroed/empty.
        valid: bool,
        /// `QScreen::name()`; empty when the platform does not name the screen.
        name: String,
        /// `QScreen::geometry()`, logical global pixels.
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        /// `QScreen::availableGeometry()`, logical global pixels.
        available_x: i32,
        available_y: i32,
        available_width: i32,
        available_height: i32,
        /// `QScreen::devicePixelRatio()`.
        scale_factor: f64,
    }

    /// `qt_screen_for_window()`: does the handle name a live window, and which
    /// screen is that window on?
    struct QtWindowScreen {
        /// false when `native` is not a handle this runtime created.
        window_known: bool,
        screen: QtScreenInfo,
    }

    /// `QCursor::pos()`: logical global position plus the scale factor of the
    /// screen under the cursor.
    struct QtCursorInfo {
        /// false when this platform cannot report a global cursor position.
        available: bool,
        x: f64,
        y: f64,
        scale_factor: f64,
    }

    unsafe extern "C++" {
        include!("qt_screen.hpp");

        /// Whether a live `QGuiApplication` exists; monitors are unknowable
        /// before one does.
        fn qt_screen_application_ready() -> bool;

        /// Every screen, in `QGuiApplication::screens()` order.
        fn qt_screen_monitors() -> Vec<QtScreenInfo>;

        /// `QGuiApplication::primaryScreen()`; `valid == false` when there is
        /// no primary screen.
        fn qt_screen_primary() -> QtScreenInfo;

        /// `QGuiApplication::screenAt()` for a logical global point;
        /// `valid == false` when no screen contains it.
        fn qt_screen_at_point(x: f64, y: f64) -> QtScreenInfo;

        /// Screen of the window behind the runtime handle `native` (the screen
        /// containing the window, read through `QWidget::windowHandle()`).
        fn qt_screen_for_window(native: u64) -> QtWindowScreen;

        /// `QCursor::pos()` in logical global coordinates.
        fn qt_screen_cursor_position() -> QtCursorInfo;
    }
}
