// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Runtime-level declarations: raw handles, event-loop proxy, exit (owned by the runtime worker).
//!
//! Add the `qt_runtime_*` functions here and implement them in `qt_runtime.cpp`.

#[cxx::bridge]
pub mod ffi {
    unsafe extern "C++" {
        include!("qt_runtime.hpp");
    }
}
