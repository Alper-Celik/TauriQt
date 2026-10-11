// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#pragma once

#include <cstdint>

#include <rust/cxx.h>

/// Window registry (qt_window.cpp). Windows are identified by a stable
/// 1-based `uint64_t` handle; the handle 0 is never handed out.
///
/// Every accessor is a no-op (or a benign default) for an unknown handle, so
/// a stale id coming from Rust can never crash the GUI thread.
///
/// Lifecycle notifications are reported through the bridge callback
/// `qt_on_window_event(id, kind, payload)` declared by the cxx-generated header
/// `tauri-runtime-qt/src/qt.rs.h`:
///
///   kind 0 - close requested      (payload empty)
///   kind 1 - focused              (payload empty)
///   kind 2 - unfocused            (payload empty)
///   kind 3 - resized              (payload "WxH", logical pixels)
///   kind 4 - moved                (payload "x,y", logical pixels)

extern "C++" {
/// Creates a top-level window on the GUI thread and returns its handle.
/// Returns 0 if no QApplication exists yet (the widget would abort otherwise).
uint64_t qt_window_create(rust::Str label, rust::Str title, int width, int height, bool visible,
                          bool decorations);

/// Shows the window (and raises/activates it).
void qt_window_show(uint64_t id);

/// Hides the window; the handle stays valid and the window can be shown again.
void qt_window_hide(uint64_t id);

/// Requests the window to close: delivers kind 0 and then hides it. The
/// handle and the widget stay alive (Qt's `close()` only hides a widget that
/// does not have `Qt::WA_DeleteOnClose` set).
void qt_window_close(uint64_t id);

/// Sets the window title.
void qt_window_set_title(uint64_t id, rust::Str title);

/// Resizes the window to `width` x `height` logical pixels.
void qt_window_set_size(uint64_t id, int width, int height);

/// Centers the window on its current screen (no-op on Wayland).
void qt_window_center(uint64_t id);

/// Toggles the window manager decorations (`Qt::FramelessWindowHint`).
void qt_window_set_decorations(uint64_t id, bool decorations);

/// Maximizes/unmaximizes through the window state.
void qt_window_set_maximized(uint64_t id, bool maximized);

/// Whether the window is currently mapped/visible.
bool qt_window_is_visible(uint64_t id);

/// Device pixel ratio of the window (`QWindow::devicePixelRatio`).
double qt_window_scale_factor(uint64_t id);

/// Native window id (`QWidget::winId()`), which Rust turns into a
/// raw-window-handle (X11 XID or Wayland `wl_surface*`).
uint64_t qt_window_winid(uint64_t id);

/// Adopts the native widget produced by the webview agent (`QWebEngineView`
/// cast to `uint64_t`) into the window, filling its client area.
void qt_window_attach_webview(uint64_t window_id, uint64_t webview_native_id);
}
