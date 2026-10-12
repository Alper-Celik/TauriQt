// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

#pragma once

#include <cstdint>

#include <rust/cxx.h>

// Runtime-level entry points: raw platform handles and the event-loop pump
// (see src/bridge/runtime.rs for the bridge declarations and
// src/ops/runtime.rs for the Rust half).
//
// Threading: entry points that touch the event loop, the application object or
// widget state are GUI-thread-only. The ones that may be called from any thread
// (`qt_runtime_display_*`, `qt_runtime_window_winid`, `qt_runtime_wake`,
// `qt_runtime_start_pump`, `qt_runtime_exit`) either read state frozen by the
// `QGuiApplication` constructor or marshal with
// `QMetaObject::invokeMethod(..., Qt::QueuedConnection)`; the value-returning
// reads wait for the GUI thread with a bounded timeout and report 0 when it did
// not answer.

/// Platform connection Qt is running on: 0 = none/unknown (no application, or
/// a platform plugin without window-system handles such as offscreen, minimal,
/// eglfs), 1 = X11 through the xcb plugin, 2 = X11 with only the Xlib
/// `Display*` available, 3 = Wayland.
std::int32_t qt_runtime_platform_kind();

/// Name of the platform plugin Qt is running on ("wayland", "xcb", "offscreen",
/// …, or "" when no QGuiApplication exists). Lets an app adapt to platform
/// limitations — on Wayland a toplevel has no global coordinates, so window
/// positions are reported as errors by design.
rust::String qt_runtime_platform_name();

/// Native display connection of the platform plugin (`xcb_connection_t*` /
/// `Display*` / `wl_display*`), 0 when there is no application or the platform
/// is not handled. Safe from any thread (immutable platform state).
std::uint64_t qt_runtime_display_pointer();

/// `QWidget::winId()` of the window registered under `id`: an XID on X11, the
/// window's `wl_surface*` on Wayland. Marshals to the GUI thread and waits up
/// to the timeout inside `qt_runtime.cpp`; 0 for an unknown handle, no
/// application, or a GUI thread that did not answer in time.
std::uint64_t qt_runtime_window_winid(std::uint64_t id);

/// Whether a `QCoreApplication` exists yet.
bool qt_runtime_has_application();

/// Posts an immediate pump tick on the Qt GUI thread. Safe and non-blocking
/// from any thread; a no-op before the `QCoreApplication` exists (queued events
/// are drained by the first tick then).
void qt_runtime_wake();

/// Installs the drain timer (16 ms, `Qt::CoarseTimer`) on the application
/// object. Idempotent; returns false when there is no `QCoreApplication`.
bool qt_runtime_start_pump();

/// `QCoreApplication::exit(code)`, marshalled to the GUI thread when needed.
void qt_runtime_exit(std::int32_t code);

/// `QCoreApplication::processEvents()`, marshalled to the GUI thread when
/// needed.
void qt_runtime_process_events();

/// Human-readable native handle state for the env-guarded proxy probe:
/// `platform=… kind=… display=0x… [winId=0x… visible=… title=…]` per top-level
/// window. GUI thread only (it calls `QWidget::winId()`).
rust::String qt_runtime_debug_native_state();
