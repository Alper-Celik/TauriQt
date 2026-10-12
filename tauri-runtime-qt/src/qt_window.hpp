// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

#pragma once

#include <cstdint>

#include <rust/cxx.h>

/// Window registry (qt_window.cpp). Windows are identified by a stable
/// 1-based `uint64_t` handle; the handle 0 is never handed out.
///
/// Every accessor is a no-op (or a benign default) for an unknown handle, so
/// a stale id coming from Rust can never crash the GUI thread. Those defaults
/// (0, false, empty string) are the "unknown window" answer and nothing else:
/// Rust checks `qt_window_alive` first whenever a wrong answer would be a lie
/// and answers `Error::WindowNotFound` instead.
///
/// Geometry crosses this boundary in Qt *device-independent* pixels
/// ("logical" px). Rust converts with `qt_window_scale_factor`; the setters
/// receive values already converted by Rust.
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

// Defined by the cxx bridge in `src/bridge/window.rs`; the generated header
// `tauri-runtime-qt/src/bridge/window.rs.h` carries the definitions.
struct QtWindowSize;
struct QtWindowPoint;

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

/// ---- window worker: registry and platform ----

/// Whether `id` names a live window in the registry.
bool qt_window_alive(uint64_t id);

/// Name of the Qt platform plugin in use ("xcb", "wayland", ...).
rust::String qt_window_platform();

/// ---- geometry (Qt device-independent pixels) ----

/// Client area size (`QWidget::size()`).
QtWindowSize qt_window_inner_size(uint64_t id);

/// Frame size including window manager decorations
/// (`QWidget::frameGeometry()`); `{0, 0}` when the platform reports no frame.
QtWindowSize qt_window_outer_size(uint64_t id);

/// Client area origin on the desktop (`QWidget::mapToGlobal(0, 0)`).
QtWindowPoint qt_window_inner_position(uint64_t id);

/// Frame origin on the desktop (`QWidget::frameGeometry().topLeft()`).
QtWindowPoint qt_window_outer_position(uint64_t id);

/// ---- state ----

/// `QWidget::windowTitle()`.
rust::String qt_window_title(uint64_t id);

/// `QStyleHints::colorScheme()` as an integer: 1 = light, 2 = dark, 0 = the
/// platform reports no colour scheme (`Qt::ColorScheme::Unknown`).
int32_t qt_window_theme(uint64_t id);

/// `QWidget::isFullScreen()` / `isMinimized()` / `isMaximized()` /
/// `isActiveWindow()` / `isEnabled()`.
bool qt_window_is_fullscreen(uint64_t id);
bool qt_window_is_minimized(uint64_t id);
bool qt_window_is_maximized(uint64_t id);
bool qt_window_is_focused(uint64_t id);
bool qt_window_is_enabled(uint64_t id);

/// Whether `Qt::FramelessWindowHint` is not set (the window has decorations).
bool qt_window_is_decorated(uint64_t id);

/// Whether the window's size hints allow resizing (`minimumSize` differs from
/// `maximumSize`); Qt exposes no separate resizable bit.
bool qt_window_is_resizable(uint64_t id);

/// Whether `Qt::WindowStaysOnTopHint` is requested. On Wayland the hint is
/// never honoured by compositors, where the flag therefore stays false.
bool qt_window_is_always_on_top(uint64_t id);

/// The button hints Qt hands to the window manager
/// (`Qt::WindowMaximizeButtonHint`, `Qt::WindowMinimizeButtonHint`,
/// `Qt::WindowCloseButtonHint`).
bool qt_window_is_maximizable(uint64_t id);
bool qt_window_is_minimizable(uint64_t id);
bool qt_window_is_closable(uint64_t id);

/// ---- setters ----

/// Enters/leaves fullscreen, restoring the maximized state the window had
/// before it went fullscreen.
void qt_window_set_fullscreen(uint64_t id, bool fullscreen);

/// `showMinimized()`; `unminimize` clears `Qt::WindowMinimized` and activates
/// the window (a Wayland compositor may refuse activation).
void qt_window_minimize(uint64_t id);
void qt_window_unminimize(uint64_t id);

/// Sets `QWidget::setMinimumSize` / `setMaximumSize` from logical pixels;
/// `0` / `QWIDGETSIZE_MAX` express "no minimum" / "no maximum".
void qt_window_set_min_size(uint64_t id, double width, double height);
void qt_window_set_max_size(uint64_t id, double width, double height);

/// Applies a window icon built from straight RGBA8888 bytes. Returns false
/// when `width * height * 4` does not match the slice length or the window is
/// unknown (Rust validates the icon before calling; this is the last line of
/// defence). Wayland has no window-icon concept and ignores the icon.
bool qt_window_set_icon(uint64_t id, rust::Slice<const uint8_t> rgba, uint32_t width,
                        uint32_t height);

/// Brings the window to the front and focuses it (`activateWindow()` +
/// `raise()`; a Wayland compositor may refuse activation).
void qt_window_set_focus(uint64_t id);

/// Toggles resizing: Qt has no resizable property, so disabling pins the
/// window to its current size with `minimumSize == maximumSize` and remembers
/// the constraints in force; re-enabling restores them.
void qt_window_set_resizable(uint64_t id, bool resizable);

/// `QWidget::setEnabled` (disables the window and everything inside it).
void qt_window_set_enabled(uint64_t id, bool enabled);

/// Sets/clears `Qt::WindowStaysOnTopHint` / `Qt::WindowStaysOnBottomHint`
/// (mutually exclusive), re-showing the window as a flag change requires.
/// Rust refuses these on platforms that cannot honour them (Wayland).
void qt_window_set_always_on_top(uint64_t id, bool on_top);
void qt_window_set_always_on_bottom(uint64_t id, bool on_bottom);

/// Paints the window's `QPalette::Window` role with the given colour and
/// resets it to the application palette when called with
/// `qt_window_reset_background_color`.
void qt_window_set_background_color(uint64_t id, uint8_t r, uint8_t g, uint8_t b, uint8_t a);
void qt_window_reset_background_color(uint64_t id);

/// Moves the frame origin to `x`, `y` logical pixels (X11 only; Rust refuses
/// platforms where clients may not place their windows).
void qt_window_set_position(uint64_t id, int x, int y);

/// `QWindow::startSystemMove()` / `startSystemResize()`; return false when Qt
/// cannot start the operation (no native window handle, or the call did not
/// come from a mouse press/release handler).
bool qt_window_start_dragging(uint64_t id);
bool qt_window_start_resize_dragging(uint64_t id, int32_t direction);
}
