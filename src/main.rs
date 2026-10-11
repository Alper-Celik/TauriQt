// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

mod ffi;

fn main() {
    let code = ffi::run_webview("https://www.google.com");
    std::process::exit(code);
}
