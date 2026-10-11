// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#pragma once

#include <cstdint>

#include <rust/cxx.h>

// The shared structs are defined by the cxx-generated header
// `tauri-runtime-qt/src/bridge/screen.rs.h`; that header includes this file
// first, so they may only be forward-declared here (the declarations below need
// nothing more than incomplete types).
struct QtScreenInfo;
struct QtWindowScreen;
struct QtCursorInfo;

/// Screen/monitor/cursor entry points (`qt_screen.cpp`), backed by
/// `QGuiApplication`/`QScreen`/`QCursor`.
///
/// All of them are callable from any thread: off-GUI-thread calls are marshalled
/// to the Qt GUI thread (the Qt platform classes below are GUI-thread-only).
/// Geometry is in Qt device-independent (logical) global pixels; the Rust side
/// multiplies by `scale_factor` to obtain physical pixels.

/// Whether a live `QGuiApplication` exists. Monitors cannot be reported before
/// one does, so callers turn `false` into an error instead of an empty list.
bool qt_screen_application_ready();

/// Every screen, in `QGuiApplication::screens()` order (the order Qt reports).
rust::Vec<QtScreenInfo> qt_screen_monitors();

/// `QGuiApplication::primaryScreen()`, or a `valid == false` value when the
/// platform has no primary screen.
QtScreenInfo qt_screen_primary();

/// `QGuiApplication::screenAt(QPoint(x, y))`. `x`/`y` are logical global
/// coordinates (the same space `QScreen::geometry()` uses). `valid == false`
/// when no screen contains the point.
QtScreenInfo qt_screen_at_point(double x, double y);

/// Screen of the window behind the runtime handle `native`: the window is
/// looked up by its native id (`qt_window_winid()`) among the top-level widgets
/// and its `QWidget::windowHandle()->screen()` is read. `window_known == false`
/// when `native` is not a live handle; `screen.valid == false` when the window
/// exists but has no screen yet.
QtWindowScreen qt_screen_for_window(std::uint64_t native);

/// `QCursor::pos()` with the scale factor of the screen under the cursor.
/// `available == false` when the platform cannot report a global cursor
/// position (Wayland, where the compositor never tells clients where the
/// pointer is) or when there is no `QGuiApplication`.
QtCursorInfo qt_screen_cursor_position();
