// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Qt-free core of `tauri-runtime-qt`: the runtime's pure logic.
//!
//! The runtime crate is a cxx bridge to Qt, so its unit-test binary cannot link
//! (`-lQt6*` lands before the C++ archive and `--as-needed` drops the Qt
//! libraries) and every behaviour check needs a live `QApplication`. Everything
//! that is *rules* rather than *plumbing* lives here instead: values in, values
//! out — no Qt object, no bridge type, no FFI, no global state.
//!
//! The split is one-sided on purpose: this crate never mentions Qt or cxx, and
//! `tauri-runtime-qt` keeps every trace line, error string and FFI call exactly
//! as it was. Moving logic here must not change a byte of the runtime's output —
//! the harnesses (`scripts/smoke.sh`, `scripts/wayland-smoke.sh`,
//! `scripts/sway-oracle.sh`) assert on those strings.
//!
//! Modules mirror the runtime's `ops` modules, plus [`scheme`] for the
//! custom-protocol bridge payloads and [`window`] for the builder mapping:
//!
//! | core module | runtime side |
//! | --- | --- |
//! | [`cookies`] | `ops::cookies` |
//! | [`scheme`] | `qt` |
//! | [`screen`] | `ops::screen` |
//! | [`webview`] | `ops::webview` |
//! | [`window`] | `ops::window`, `lib.rs` builder impls |

#![forbid(unsafe_code)]

pub mod cookies;
pub mod scheme;
pub mod screen;
pub mod webview;
pub mod window;
