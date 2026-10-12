// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Per-area implementations behind the trait impls.
//!
//! Each module is owned by exactly one worker. The trait impls in `lib.rs` are
//! thin delegations, so a worker never has to touch a shared file.
//!
//! Contract: a function returns `Err(Error::CreateWindow/CreateWebview(...))`
//! when it cannot answer *truthfully*. Never return a fabricated value (a made
//! up `false`, `0` or empty string) — callers branch on these.

pub mod cookies;
pub mod runtime;
pub mod screen;
pub mod webview;
pub mod window;

use tauri_runtime::Error;

/// The error every not-yet-implemented operation returns.
pub fn unimplemented(what: &str) -> Error {
    Error::CreateWebview(format!("tauri-runtime-qt: {what} is not implemented yet").into())
}
