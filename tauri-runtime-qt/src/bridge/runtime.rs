// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Runtime-level declarations (owned by the runtime worker): raw platform
//! handles, the event-loop pump and exit.
//!
//! C++ translation units include the cxx-generated header for this bridge:
//! `#include "tauri-runtime-qt/src/bridge/runtime.rs.h"`. It pulls in
//! `qt_runtime.hpp` and declares the `qt_on_pump_tick` callback with the
//! linkage cxx expects, so `qt_runtime.cpp` must not hand-declare it.

#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("qt_runtime.hpp");

        // ---- native platform / raw handles (qt_runtime.cpp) ----
        /// Platform connection Qt is running on: 0 = none/unknown (no
        /// `QGuiApplication`, or a platform plugin without window-system
        /// handles such as offscreen/minimal/eglfs), 1 = X11 through the xcb
        /// plugin, 2 = X11 with only the Xlib `Display*` available, 3 =
        /// Wayland.
        fn qt_runtime_platform_kind() -> i32;

        /// Name of the platform plugin Qt runs on ("wayland", "xcb", …), empty
        /// when there is no `QGuiApplication`.
        fn qt_runtime_platform_name() -> String;

        /// Native display connection of the platform plugin
        /// (`xcb_connection_t*` / `Display*` / `wl_display*`), 0 when there is
        /// no application or the platform is not handled. Reads only state the
        /// `QGuiApplication` constructor froze, so it is safe from any thread.
        fn qt_runtime_display_pointer() -> u64;

        /// Native window id (`QWidget::winId()`) of the window registered under
        /// `id`: an XID on X11, the `wl_surface*` of the window on Wayland.
        /// Marshals to the GUI thread and waits (bounded, see
        /// `qt_runtime.hpp`); returns 0 for an unknown handle or when the GUI
        /// thread did not answer in time.
        fn qt_runtime_window_winid(id: u64) -> u64;

        // ---- event loop ----
        /// Posts an immediate pump tick on the Qt GUI thread. Safe and
        /// non-blocking from any thread; a no-op before the `QCoreApplication`
        /// exists (the events stay queued until the first tick).
        fn qt_runtime_wake();

        /// Whether a `QCoreApplication` exists (the runtime has been
        /// constructed); `request_exit` has nothing to exit otherwise.
        fn qt_runtime_has_application() -> bool;

        /// Installs the drain timer on the application object (idempotent).
        /// Returns false when there is no `QCoreApplication`.
        fn qt_runtime_start_pump() -> bool;

        /// `QCoreApplication::exit(code)` on the GUI thread.
        fn qt_runtime_exit(code: i32);

        /// `QCoreApplication::processEvents()` on the GUI thread.
        fn qt_runtime_process_events();

        /// Human-readable native handle state ("platform=… kind=… display=…
        /// [winId=…]") for the env-guarded proxy probe in `ops::runtime`.
        /// GUI thread only (it calls `QWidget::winId()`).
        fn qt_runtime_debug_native_state() -> String;
    }

    extern "Rust" {
        /// Drain hook: called by the pump timer and by `qt_runtime_wake()` on
        /// the Qt GUI thread. Feeds the queued events to the sink and leaves the
        /// loop when an exit was requested. The logic lives in
        /// `ops::runtime::pump_tick`.
        fn qt_on_pump_tick();
    }
}

/// Drain hook called from Qt on the GUI thread (see the bridge declaration).
pub fn qt_on_pump_tick() {
    crate::ops::runtime::pump_tick();
}
