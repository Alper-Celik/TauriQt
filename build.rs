// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

use cxx_qt_build::CxxQtBuilder;

fn main() {
    // SAFETY: callback only registers an include directory; no bridge state is touched.
    let builder = unsafe {
        CxxQtBuilder::new()
            .qt_module("Widgets")
            .qt_module("WebEngineWidgets")
            .files(["src/ffi.rs"])
            .cc_builder(|cc| {
                cc.include("src");
            })
    };
    builder.build();
}
