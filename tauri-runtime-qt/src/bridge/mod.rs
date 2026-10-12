// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One cxx bridge per area, so each worker owns exactly one declaration file.

pub mod cookies;
pub mod runtime;
pub mod screen;
pub mod webview;
pub mod window;
