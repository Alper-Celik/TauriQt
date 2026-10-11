// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Webview declarations (owned by the webview worker).
//!
//! C++ translation units include the cxx-generated header for this bridge:
//! `#include "tauri-runtime-qt/src/bridge/webview.rs.h"`.

#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("qt_webview.hpp");

        // ---- webviews (qt_webview.cpp) ----
        fn qt_webview_create(window_id: u64, url: &str) -> u64;
        /// Injects a tauri initialization script at document creation.
        fn qt_webview_add_init_script(id: u64, script: &str, main_frame_only: bool);
        fn qt_webview_native_handle(id: u64) -> u64;
        fn qt_webview_navigate(id: u64, url: &str);
        fn qt_webview_url(id: u64) -> String;
        fn qt_webview_reload(id: u64);
        fn qt_webview_eval(id: u64, script: &str);
        fn qt_webview_set_zoom(id: u64, factor: f64);
        fn qt_webview_set_background_color(id: u64, r: i32, g: i32, b: i32, a: i32);
        fn qt_webview_show(id: u64);
        fn qt_webview_hide(id: u64);
        fn qt_webview_set_bounds(id: u64, x: i32, y: i32, width: i32, height: i32);
        fn qt_webview_open_devtools(id: u64);
        fn qt_webview_close_devtools(id: u64);
        fn qt_webview_is_devtools_open(id: u64) -> bool;
        fn qt_webview_print(id: u64);
        fn qt_webview_close(id: u64);

    }
}
