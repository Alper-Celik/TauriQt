// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Runtime tracing.
//!
//! The harnesses (`scripts/smoke.sh`, `scripts/wayland-smoke.sh`,
//! `scripts/sway-oracle.sh`) assert on `[tauri-runtime-qt] …` lines, but the
//! packaged app is a **release** build — so gating those lines on
//! `debug_assertions` meant CI asserted against output that a packaged app never
//! prints. Opt in at runtime with `TAURIQT_TRACE=1` instead; the flake's check
//! environment sets it, and a debug build behaves as before.

/// True when `TAURIQT_TRACE` is set. Cached: the value cannot change usefully
/// mid-process and this is consulted on hot paths.
pub fn trace_enabled() -> bool {
    static ENABLED: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var_os("TAURIQT_TRACE").is_some());
    *ENABLED
}

/// `eprintln!` when [`trace_enabled`] — no formatting cost when tracing is off.
#[macro_export]
macro_rules! qtrace {
    ($($arg:tt)*) => {
        if $crate::trace::trace_enabled() {
            eprintln!($($arg)*);
        }
    };
}
