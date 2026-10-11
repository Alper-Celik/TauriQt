// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Screen/monitor/cursor declarations (owned by the screen worker).
//!
//! Add the `qt_screen_*` functions here and implement them in `qt_screen.cpp`
//! (`build.rs` picks new .cpp files up automatically).

#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("qt_screen.hpp");
    }
}
