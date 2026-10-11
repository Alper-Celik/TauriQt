// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Window declarations (owned by the window worker).
//!
//! C++ translation units include the cxx-generated header for this bridge:
//! `#include "tauri-runtime-qt/src/bridge/window.rs.h"`.

#[cxx::bridge]
pub mod ffi {
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

    }
}
