// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! One cxx bridge per area, so each worker owns exactly one declaration file.

pub mod runtime;
pub mod screen;
pub mod webview;
pub mod window;
