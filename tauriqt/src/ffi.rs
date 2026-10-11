// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#[cxx_qt::bridge]
pub mod ffi {
    #[namespace = ""]
    unsafe extern "C++" {
        include!("webview.hpp");

        fn run_webview(url: &str) -> i32;
    }
}
