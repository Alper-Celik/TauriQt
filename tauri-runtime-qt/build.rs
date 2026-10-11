// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Build the Rust <-> Qt bridge with plain `cxx` and let `qt-build-utils`
//! resolve the Qt module flags (it is what cxx-qt uses internally, without the
//! QObject/initializer machinery this crate does not need).

use qt_build_utils::QtBuild;

fn main() {
    let qt = QtBuild::new(vec!["Widgets".to_owned(), "WebEngineWidgets".to_owned()])
        .expect("failed to locate a usable Qt installation (is QMAKE set?)");

    let cpp_sources = [
        "src/qt_app.cpp",
        "src/qt_window.cpp",
        "src/qt_webview.cpp",
        "src/qt_scheme.cpp",
    ];

    let mut build = cxx_build::bridge("src/qt.rs");
    build
        .files(cpp_sources)
        .include("src")
        .includes(qt.include_paths())
        .std("c++17");
    qt.cargo_link_libraries(&mut build);
    build.compile("tauri-runtime-qt-bridge");

    println!("cargo::rerun-if-changed=src/qt.rs");
    println!("cargo::rerun-if-changed=src/qt_app.hpp");
    println!("cargo::rerun-if-changed=src/qt_app.cpp");
    println!("cargo::rerun-if-changed=src/qt_window.hpp");
    println!("cargo::rerun-if-changed=src/qt_window.cpp");
    println!("cargo::rerun-if-changed=src/qt_webview.hpp");
    println!("cargo::rerun-if-changed=src/qt_webview.cpp");
    println!("cargo::rerun-if-changed=src/qt_scheme.hpp");
    println!("cargo::rerun-if-changed=src/qt_scheme.cpp");
}
