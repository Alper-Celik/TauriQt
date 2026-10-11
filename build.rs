// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

use cxx_qt_build::CxxQtBuilder;

fn main() {
    // SAFETY: callback only registers include directory and an extra C++ file;
    // no bridge state is touched.
    let builder = unsafe {
        CxxQtBuilder::new()
            .qt_module("Widgets")
            .qt_module("WebEngineWidgets")
            .files(["src/ffi.rs"])
            .cc_builder(|cc| {
                cc.include("src");
                cc.file("src/webview.cpp");
            })
    };
    builder.build();
}
