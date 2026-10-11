// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Runtime-level operations (owned by the runtime worker): raw window/display
//! handles, the cross-thread event proxy and event-loop control.

#![allow(unused_variables)]

use raw_window_handle::{DisplayHandle, HandleError, WindowHandle};
use tauri_runtime::{Result, UserEvent};

use super::unimplemented;

/// One queued user event per `EventLoopProxy::send_event`, drained by the Qt
/// event loop. The worker owns both halves (queue + drain hook).
pub(crate) fn proxy_send_event<T: UserEvent>(event: T) -> Result<()> {
    Err(unimplemented("EventLoopProxy::send_event"))
}

/// Requests an exit of the Qt event loop with the given code.
pub(crate) fn request_exit(code: i32) -> Result<()> {
    Err(unimplemented("RuntimeHandle::request_exit"))
}

/// Runs one turn of the Qt event loop without blocking.
pub(crate) fn run_iteration() {
    // The worker wires this to `QCoreApplication::processEvents`.
}

pub(crate) fn window_handle(native: u64) -> std::result::Result<WindowHandle<'static>, HandleError> {
    Err(HandleError::Unavailable)
}

pub(crate) fn display_handle() -> std::result::Result<DisplayHandle<'static>, HandleError> {
    Err(HandleError::Unavailable)
}
