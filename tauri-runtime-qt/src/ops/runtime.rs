// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! Runtime-level operations (owned by the runtime worker): raw window/display
//! handles, the cross-thread event proxy and event-loop control.
//!
//! # Event flow
//!
//! `EventLoopProxy::send_event` boxes the event into a process-wide queue and
//! wakes the Qt event loop. On the GUI thread the pump tick (`qt_on_pump_tick`
//! in `crate::bridge::runtime`, driven by the `QTimer` installed by
//! `start_pump()` and by the queued wake-ups) drains the queue into the sink
//! installed with `set_event_sink()`. `request_exit()` stores the exit code; the
//! tick delivers the events queued before it and then leaves the loop through
//! `QCoreApplication::exit(code)`.
//!
//! # Raw handles
//!
//! What Qt can answer truthfully (verified against Qt 6.11 sources):
//!
//! * X11 (xcb plugin): `QWidget::winId()` is the window's XID and
//!   `QX11Application` hands out the `xcb_connection_t*` (`Display*` fallback).
//! * Wayland: `QWidget::winId()` is the window's `wl_surface*`
//!   (`QWaylandWindow::winId()` returns `wlSurface()`, qtbase
//!   `src/plugins/platforms/wayland/qwaylandwindow.cpp`), and
//!   `QWaylandApplication` hands out the `wl_display*`.
//! * Every other platform plugin (offscreen, minimal, eglfs, …) and a missing
//!   application: no window handle exists — the offscreen plugin even answers
//!   `QX11Application` with a GLX-only `Display*` and returns counter values
//!   from `winId()` — so `HandleError::Unavailable` is returned instead of a
//!   fabricated value. The plugin name gates the decision (`platformConnection`
//!   in `qt_runtime.cpp`).

use std::any::Any;
use std::collections::VecDeque;
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::os::raw::{c_int, c_ulong};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicI32, Ordering};

use parking_lot::Mutex;
use raw_window_handle::{
    DisplayHandle, HandleError, RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle,
    WaylandWindowHandle, WindowHandle, XcbDisplayHandle, XcbWindowHandle, XlibDisplayHandle,
    XlibWindowHandle,
};
use tauri_runtime::{Error, Result, UserEvent};

use crate::bridge::runtime::ffi;

/// Platform kinds reported by `qt_runtime_platform_kind()` (see the bridge);
/// anything else — including 0, "no application / unsupported platform" — maps
/// to `NativePlatform::None`.
const PLATFORM_XCB: i32 = 1;
const PLATFORM_XLIB: i32 = 2;
const PLATFORM_WAYLAND: i32 = 3;

/// `EXIT_CODE` value meaning "no exit requested".
const NO_EXIT: i32 = i32::MIN;

/// X11 screen index reported in display handles. Qt's `QX11Application` exposes
/// `display()` and `connection()` only — no screen number — and Qt connects to
/// the connection's default screen, which is index 0.
const X11_DEFAULT_SCREEN: c_int = 0;

/// Events queued by `proxy_send_event`, drained on the Qt GUI thread.
static EVENT_QUEUE: Mutex<VecDeque<Box<dyn Any + Send>>> = Mutex::new(VecDeque::new());

/// Where drained events go; installed by the integrator from `run()`.
static EVENT_SINK: Mutex<Option<Box<dyn FnMut(Box<dyn Any + Send>) + Send>>> = Mutex::new(None);

/// Pending exit code (see `request_exit`).
static EXIT_CODE: AtomicI32 = AtomicI32::new(NO_EXIT);

/// Queues `event` for delivery on the Qt GUI thread.
///
/// Safe and non-blocking from any thread, and usable before the event loop
/// runs (the queue is drained by the first pump tick).
pub(crate) fn proxy_send_event<T: UserEvent>(event: T) -> Result<()> {
    enqueue(Box::new(event));
    Ok(())
}

/// Pushes a boxed event into the queue and schedules a drain.
///
/// One wake-up is posted per event: the GUI thread processes the posted
/// invocation after the event was pushed by this thread, so the drain it
/// triggers always sees the event. (Coalescing the wake-ups would risk losing
/// the last one; an extra queued invocation is cheap.)
fn enqueue(event: Box<dyn Any + Send>) {
    EVENT_QUEUE.lock().push_back(event);
    ffi::qt_runtime_wake();
}

/// Installs the sink that receives every drained event.
///
/// The integrator calls this once from `run()`/`run_return()`; calling it again
/// replaces the previous sink. The sink is invoked exclusively on the Qt GUI
/// thread (from the pump tick) and must not call `set_event_sink` itself — use
/// `main_thread_sink` when the closure is not `Send`.
pub fn set_event_sink(sink: Box<dyn FnMut(Box<dyn Any + Send>) + Send>) {
    let sink = debug::wrap_sink(sink);
    *EVENT_SINK.lock() = Some(sink);
}

/// Closure wrapper that is only ever invoked on the Qt GUI thread but may be
/// stored in a `Send` slot (see `main_thread_sink`).
struct MainThreadOnly<F>(F);

// SAFETY: the pump tick — see `main_thread_sink` — is the only caller and it
// runs on the Qt GUI thread.
unsafe impl<F> Send for MainThreadOnly<F> {}

impl<F: FnMut(Box<dyn Any + Send>)> MainThreadOnly<F> {
    fn call(&mut self, event: Box<dyn Any + Send>) {
        (self.0)(event)
    }
}

/// Wraps a closure that is only ever invoked on the Qt GUI thread so it can be
/// handed to `set_event_sink`, which requires `Send` for its process-wide slot.
///
/// The trait's `run`/`run_return` take `FnMut(RunEvent<T>)` without a `Send`
/// bound, so the callback may capture `Rc`/`RefCell` state; the wrapper asserts
/// that this never matters because the pump drains on the GUI thread only.
pub fn main_thread_sink<F>(sink: F) -> Box<dyn FnMut(Box<dyn Any + Send>) + Send>
where
    F: FnMut(Box<dyn Any + Send>) + 'static,
{
    // Calling through the wrapper (instead of `sink.0`) keeps the closure from
    // capturing the inner `F` by itself, which would require `F: Send`.
    let mut sink = MainThreadOnly(sink);
    Box::new(move |event| sink.call(event))
}

/// Installs the Qt-side drain timer; called once from `run()`/`run_return()`
/// after `set_event_sink`.
pub fn start_pump() {
    if !ffi::qt_runtime_start_pump() {
        #[cfg(debug_assertions)]
        eprintln!("[tauri-runtime-qt] start_pump: no QApplication yet, events stay queued");
    }
    debug::fire_probe();
}

/// Requests an exit of the Qt event loop with `code`.
///
/// Safe from any thread and non-blocking: the code is stored and the GUI thread
/// exits on the next pump tick, after delivering the events that were queued
/// before this call. Returns `Err(Error::EventLoopClosed)` when no
/// `QCoreApplication` exists (nothing to exit); events queued before `run()`
/// start are still delivered, because the first tick exits.
pub(crate) fn request_exit(code: i32) -> Result<()> {
    if !ffi::qt_runtime_has_application() {
        return Err(Error::EventLoopClosed);
    }
    EXIT_CODE.store(code, Ordering::SeqCst);
    ffi::qt_runtime_wake();
    Ok(())
}

/// Runs one turn of the Qt event loop without blocking.
pub(crate) fn run_iteration() {
    ffi::qt_runtime_process_events();
}

/// Pump tick; runs on the Qt GUI thread (see `bridge::runtime::qt_on_pump_tick`).
pub(crate) fn pump_tick() {
    // Take the queue under the lock but feed the sink outside of it: the sink
    // may send further events through the proxy, and the queue mutex is not
    // reentrant.
    let drained: Vec<Box<dyn Any + Send>> = EVENT_QUEUE.lock().drain(..).collect();

    if !drained.is_empty() {
        let mut slot = EVENT_SINK.lock();
        match slot.as_mut() {
            Some(sink) => {
                for event in drained {
                    sink(event);
                }
            }
            // No sink yet (`run()` has not started): hold the events for the
            // next tick instead of dropping them.
            None => {
                let mut queue = EVENT_QUEUE.lock();
                for event in drained.into_iter().rev() {
                    queue.push_front(event);
                }
            }
        }
    }

    // Exit last: everything queued before the exit request was delivered above.
    let code = EXIT_CODE.swap(NO_EXIT, Ordering::SeqCst);
    if code != NO_EXIT {
        ffi::qt_runtime_exit(code);
    }
}

/// Native platform Qt is running on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativePlatform {
    /// No QGuiApplication yet, or a plugin without native handles.
    None,
    /// X11 through the xcb plugin: XID + `xcb_connection_t*`.
    Xcb,
    /// X11 with only the Xlib `Display*` available.
    Xlib,
    /// Wayland: `wl_surface*` + `wl_display*`.
    Wayland,
}

fn native_platform() -> NativePlatform {
    match ffi::qt_runtime_platform_kind() {
        PLATFORM_XCB => NativePlatform::Xcb,
        PLATFORM_XLIB => NativePlatform::Xlib,
        PLATFORM_WAYLAND => NativePlatform::Wayland,
        _ => NativePlatform::None,
    }
}

/// Raw handle of the Qt window behind the runtime's `native` handle.
///
/// X11: `QWidget::winId()` is the XID; the xcb variant is used when Qt has an
/// xcb connection, the Xlib variant otherwise. Wayland: `QWidget::winId()` is
/// the window's `wl_surface*`. Unknown handles, missing windows and platforms
/// without native handles report `HandleError::Unavailable`.
pub(crate) fn window_handle(native: u64) -> std::result::Result<WindowHandle<'static>, HandleError> {
    let platform = native_platform();
    let winid = ffi::qt_runtime_window_winid(native);

    let raw = match platform {
        NativePlatform::Xcb => {
            let window = u32::try_from(winid)
                .ok()
                .and_then(NonZeroU32::new)
                .ok_or(HandleError::Unavailable)?;
            RawWindowHandle::Xcb(XcbWindowHandle::new(window))
        }
        NativePlatform::Xlib => {
            if winid == 0 {
                return Err(HandleError::Unavailable);
            }
            RawWindowHandle::Xlib(XlibWindowHandle::new(winid as c_ulong))
        }
        NativePlatform::Wayland => {
            let surface = NonNull::new(winid as *mut c_void).ok_or(HandleError::Unavailable)?;
            RawWindowHandle::Wayland(WaylandWindowHandle::new(surface))
        }
        NativePlatform::None => return Err(HandleError::Unavailable),
    };

    // SAFETY: the handle describes the live Qt platform resource. Qt keeps it
    // alive for at least as long as the window exists, and the returned borrow
    // never outlives the process-lifetime QGuiApplication that owns it.
    Ok(unsafe { WindowHandle::borrow_raw(raw) })
}

/// Raw display handle of the Qt platform connection.
///
/// The pointers come from the `QNativeInterface` the platform plugin
/// implements (`QX11Application` / `QWaylandApplication`); platforms without
/// one report `HandleError::Unavailable`.
pub(crate) fn display_handle() -> std::result::Result<DisplayHandle<'static>, HandleError> {
    let platform = native_platform();
    let pointer = ffi::qt_runtime_display_pointer();

    let raw = match platform {
        NativePlatform::Xcb => {
            let connection = NonNull::new(pointer as *mut c_void).ok_or(HandleError::Unavailable)?;
            RawDisplayHandle::Xcb(XcbDisplayHandle::new(Some(connection), X11_DEFAULT_SCREEN))
        }
        NativePlatform::Xlib => {
            let display = NonNull::new(pointer as *mut c_void).ok_or(HandleError::Unavailable)?;
            RawDisplayHandle::Xlib(XlibDisplayHandle::new(Some(display), X11_DEFAULT_SCREEN))
        }
        NativePlatform::Wayland => {
            let display = NonNull::new(pointer as *mut c_void).ok_or(HandleError::Unavailable)?;
            RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display))
        }
        NativePlatform::None => return Err(HandleError::Unavailable),
    };

    // SAFETY: the connection is owned by the Qt platform plugin and lives until
    // the process exits; the borrow does not outlive it.
    Ok(unsafe { DisplayHandle::borrow_raw(raw) })
}

/// `TAURIQT_PROXY_DEBUG=1` end-to-end probe of the proxy path. It logs every
/// event the sink receives, queues three probe events from a worker thread at
/// pump start and then asks the loop to `exit(0)`. The whole module is dormant
/// unless the environment variable is set.
mod debug {
    use super::*;
    use std::sync::Once;
    use std::time::Duration;

    static PROBE: Once = Once::new();

    pub fn enabled() -> bool {
        std::env::var_os("TAURIQT_PROXY_DEBUG").is_some_and(|value| value != "0")
    }

    /// Logs each event on its way into the real sink.
    pub fn wrap_sink(
        mut sink: Box<dyn FnMut(Box<dyn Any + Send>) + Send>,
    ) -> Box<dyn FnMut(Box<dyn Any + Send>) + Send> {
        if !enabled() {
            return sink;
        }
        let mut received = 0usize;
        Box::new(move |event| {
            received += 1;
            match event.downcast_ref::<String>() {
                Some(payload) => eprintln!(
                    "[tauri-runtime-qt] proxy-debug: sink received event #{received}: {payload}"
                ),
                None => eprintln!(
                    "[tauri-runtime-qt] proxy-debug: sink received event #{received} (non-probe payload)"
                ),
            }
            sink(event);
        })
    }

    /// Queues three probe events from a worker thread, then requests exit(0).
    pub fn fire_probe() {
        if !enabled() {
            return;
        }
        PROBE.call_once(|| {
            eprintln!(
                "[tauri-runtime-qt] proxy-debug: native state: {}",
                ffi::qt_runtime_debug_native_state()
            );
            eprintln!(
                "[tauri-runtime-qt] proxy-debug: display_handle() = {:?}",
                display_handle()
            );
            eprintln!(
                "[tauri-runtime-qt] proxy-debug: window_handle(u64::MAX) = {:?}",
                window_handle(u64::MAX)
            );
            // The registry hands out 1-based handles; report the first real
            // window as well (unknown ids answer with `Unavailable`).
            for id in 1..=3u64 {
                if let Ok(handle) = window_handle(id) {
                    eprintln!("[tauri-runtime-qt] proxy-debug: window_handle({id}) = {handle:?}");
                    break;
                }
            }
            eprintln!("[tauri-runtime-qt] proxy-debug: queueing 3 probe events from a worker thread");
            std::thread::spawn(|| {
                for index in 1..=3 {
                    std::thread::sleep(Duration::from_millis(200));
                    eprintln!("[tauri-runtime-qt] proxy-debug: worker queues probe #{index}");
                    enqueue(Box::new(format!("proxy-probe-{index}")));
                }
                std::thread::sleep(Duration::from_millis(400));
                eprintln!("[tauri-runtime-qt] proxy-debug: worker requests exit(0)");
                let _ = request_exit(0);
            });
        });
    }
}
