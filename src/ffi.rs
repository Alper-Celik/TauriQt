// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

#[cxx_qt::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("webview.hpp");

        pub fn run_webview(url: &str) -> i32;
    }
}
