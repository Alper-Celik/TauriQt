// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Build the Rust <-> Qt bridge with plain `cxx` and let `qt-build-utils`
//! resolve the Qt module flags (it is what cxx-qt uses internally, without the
//! QObject/initializer machinery this crate does not need).
//!
//! Bridge files and C++ sources are discovered by glob so that adding a worker's
//! file never edits this script.

use std::path::{Path, PathBuf};

use qt_build_utils::QtBuild;

/// All `.rs` files under `src/` that declare a `#[cxx::bridge]`, sorted.
fn bridge_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(&Path::new("src").to_path_buf(), &mut files, |path| {
        path.extension().is_some_and(|ext| ext == "rs")
            && std::fs::read_to_string(path)
                .map(|text| text.contains("#[cxx::bridge]"))
                .unwrap_or(false)
    });
    files.sort();
    files
}

/// All `.cpp` translation units under `src/`, sorted.
fn cpp_sources() -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect(&Path::new("src").to_path_buf(), &mut files, |path| {
        path.extension().is_some_and(|ext| ext == "cpp")
    });
    files.sort();
    files
}

fn collect(dir: &PathBuf, out: &mut Vec<PathBuf>, keep: impl Fn(&PathBuf) -> bool + Copy) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out, keep);
        } else if keep(&path) {
            out.push(path);
        }
    }
}

fn main() {
    let qt = QtBuild::new(vec!["Widgets".to_owned(), "WebEngineWidgets".to_owned()])
        .expect("failed to locate a usable Qt installation (is QMAKE set?)");

    let bridges = bridge_files();
    let sources = cpp_sources();
    assert!(
        !bridges.is_empty() && !sources.is_empty(),
        "no bridges/sources found under src/"
    );

    let mut build = cxx_build::bridges(&bridges);
    build
        .files(&sources)
        .include("src")
        .includes(qt.include_paths())
        .std("c++17");
    qt.cargo_link_libraries(&mut build);
    build.compile("tauri-runtime-qt-bridge");

    for file in bridges.iter().chain(sources.iter()) {
        println!("cargo::rerun-if-changed={}", file.display());
    }
}
