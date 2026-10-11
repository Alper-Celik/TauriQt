// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! A Qt runtime for Tauri 2 on Linux: Qt windowing replaces tao, QtWebEngine
//! replaces WebKitGTK. The crate implements `tauri_runtime::Runtime`, so an app
//! selects it at the type level (`tauri::Builder::<QtRuntime>::new()`) and does
//! not enable tauri's `wry` feature on Linux. See README.md for the wiring an
//! app has to add, and RUNTIME-TRAITS.md for the Qt mapping of every trait
//! method.
//!
//! Two items outside the stock trait surface exist for the app to use:
//!
//! - [`INVOKE_SYSTEM_SCRIPT`]: QtWebEngine never delivers custom-protocol POST
//!   bodies, so the invoke payload travels in a header instead. Pass it to
//!   `tauri::Builder::invoke_system`.
//! - [`platform_name`]: which Qt platform plugin is running (`"wayland"`,
//!   `"xcb"`, `"offscreen"`, …), for apps that must adapt to platform gaps —
//!   a Wayland toplevel has no global coordinates, so the position getters and
//!   `set_position` return errors there by design (docs/nix-qt-gotchas.md).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use parking_lot::Mutex;
use raw_window_handle::{DisplayHandle, HandleError, WindowHandle};
use tauri_runtime as rt;
use tauri_runtime::{
    monitor::Monitor,
    webview::{DetachedWebview, PendingWebview},
    window::{
        CursorIcon, DetachedWindow, PendingWindow, RawWindow, WindowEvent, WindowId,
        WindowSizeConstraints, WebviewEvent,
    },
    Cookie, DeviceEventFilter, Error, EventLoopProxy, Icon, ProgressBarState, ResizeDirection,
    Result, RunEvent, RuntimeInitArgs, UserAttentionType, UserEvent, WebviewEventId, WindowEventId,
};
use tauri_utils::config::{Color, WindowConfig};
use tauri_utils::Theme;

pub mod bridge;
pub mod trace;

/// Name of the platform plugin Qt is running on ("wayland", "xcb", "offscreen",
/// …), or an empty string before the application exists.
///
/// Apps use this to adapt to platform limitations rather than guess: on Wayland
/// a toplevel has no global coordinates, so `inner_position`/`outer_position`
/// and `set_position` return errors by design (see `docs/nix-qt-gotchas.md`).
pub fn platform_name() -> String {
    ops::runtime::platform_name()
}
pub mod ops;
pub mod qt;

/// Invoke system for this runtime.
///
/// QtWebEngine never delivers custom-protocol POST bodies, so tauri's default
/// invoke system (payload in the request body) cannot work here. This script
/// keeps the custom-protocol path but moves the payload into the
/// `X-Tauriqt-Payload` header (base64), which the runtime turns back into the
/// request body before dispatching to tauri's IPC handler.
///
/// Use it with `tauri::Builder::invoke_system(tauri_runtime_qt::INVOKE_SYSTEM_SCRIPT)`.
pub const INVOKE_SYSTEM_SCRIPT: &str = include_str!("invoke_system.js");

/// The Qt runtime. Constructed on the main thread; owns the `QApplication`.
#[derive(Debug, Clone, Copy, Default)]
pub struct QtRuntime;

/// Thread-safe handle to the runtime.
#[derive(Debug, Clone, Copy)]
pub struct QtRuntimeHandle;

/// Cross-thread user-event proxy.
#[derive(Debug, Clone, Copy)]
pub struct QtEventLoopProxy;

/// Per-window dispatcher: native Qt window handle (from `qt_window_create`).
#[derive(Debug, Clone, Copy)]
pub struct QtWindowDispatcher {
    pub(crate) native: u64,
}

/// Per-webview dispatcher: native Qt webview handle (from `qt_webview_create`).
#[derive(Debug, Clone)]
pub struct QtWebviewDispatcher {
    pub(crate) native: u64,
    pub(crate) label: String,
}

/// Window event handler as registered through `on_window_event`.
type WindowEventHandler = Box<dyn Fn(&WindowEvent) + Send + 'static>;
/// Webview event handler as registered through `on_webview_event`.
type WebviewEventHandler = Box<dyn Fn(&WebviewEvent) + Send + 'static>;

/// `(event id, native window handle, handler)`.
static WINDOW_HANDLERS: Mutex<Vec<(WindowEventId, u64, WindowEventHandler)>> = Mutex::new(Vec::new());
/// `(event id, native webview handle, handler)`.
static WEBVIEW_HANDLERS: Mutex<Vec<(WebviewEventId, u64, WebviewEventHandler)>> =
    Mutex::new(Vec::new());
/// Monotonic id shared by both registries.
static NEXT_EVENT_ID: AtomicU32 = AtomicU32::new(1);

/// Registers a window event handler for a native window handle.
fn register_window_handler<F: Fn(&WindowEvent) + Send + 'static>(native: u64, f: F) -> WindowEventId {
    let id = NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed);
    WINDOW_HANDLERS.lock().push((id, native, Box::new(f)));
    id
}

/// Registers a webview event handler for a native webview handle.
fn register_webview_handler<F: Fn(&WebviewEvent) + Send + 'static>(
    native: u64,
    f: F,
) -> WebviewEventId {
    let id = NEXT_EVENT_ID.fetch_add(1, Ordering::Relaxed);
    WEBVIEW_HANDLERS.lock().push((id, native, Box::new(f)));
    id
}

/// Native handle of the Qt window backing each `WindowId`.
static NATIVE_WINDOWS: Mutex<Vec<(WindowId, u64)>> = Mutex::new(Vec::new());

/// Custom-protocol handler as tauri registers it on a pending webview.
type SchemeHandler = Box<
    dyn Fn(&str, http::Request<Vec<u8>>, Box<dyn FnOnce(http::Response<std::borrow::Cow<'static, [u8]>>) + Send>)
        + Send
        + Sync
        + 'static,
>;

/// Per native webview handle: its tauri label plus the `uri_scheme_protocols`
/// tauri registered for it (scheme name → handler). Tauri's handlers take the
/// webview *label*, not the scheme, as their first argument.
static SCHEME_PROTOCOLS: Mutex<Vec<(u64, String, HashMap<String, SchemeHandler>)>> =
    Mutex::new(Vec::new());

/// Creates the Qt window for a pending window and records its handle.
fn build_window<T: UserEvent>(
    label: String,
    builder: QtWindowBuilder,
) -> Result<(WindowId, QtWindowDispatcher)> {
    let native = bridge::window::ffi::qt_window_create(
        &label,
        &builder.title,
        builder.width as i32,
        builder.height as i32,
        builder.visible,
        builder.decorations,
    );
    if native == 0 {
        return Err(Error::CreateWindow);
    }
    if builder.maximized {
        bridge::window::ffi::qt_window_set_maximized(native, true);
    }
    if builder.center {
        bridge::window::ffi::qt_window_center(native);
    }
    if let Some((x, y)) = builder.position {
        let _ = (x, y); // M4: absolute positioning is a no-op on Wayland.
    }
    let id = WindowId::from(native as u32);
    NATIVE_WINDOWS.lock().push((id, native));
        crate::qtrace!("[tauri-runtime-qt] window '{label}' created (native {native})");
    Ok((id, QtWindowDispatcher { native }))
}

/// Creates the Qt webview inside an existing window and installs the schemes.
fn build_webview<T: UserEvent>(
    window_native: u64,
    pending: PendingWebview<T, QtRuntime>,
) -> Result<DetachedWebview<T, QtRuntime>> {
    let label = pending.label.clone();
    let pending_url = pending.url.clone();
    // Create the view without navigating, inject tauri's initialization scripts
    // (they define `window.__TAURI_INTERNALS__`, i.e. `invoke`), then navigate.
    let native = bridge::webview::ffi::qt_webview_create(window_native, "");
    if native == 0 {
        return Err(Error::CreateWebview(
            "qt_webview_create returned a null handle".into(),
        ));
    }
    let view_ptr = bridge::webview::ffi::qt_webview_native_handle(native);
    bridge::window::ffi::qt_window_attach_webview(window_native, view_ptr);
    SCHEME_PROTOCOLS
        .lock()
        .push((native, label.clone(), pending.uri_scheme_protocols));
    qt::ffi::qt_scheme_install_handler(native);
    // Mirror the profile's cookie store (Qt can only enumerate asynchronously).
    ops::cookies::register_webview(native, view_ptr);

    for script in &pending.webview_attributes.initialization_scripts {
        bridge::webview::ffi::qt_webview_add_init_script(native, &script.script, script.for_main_frame_only);
    }
    if !pending_url.is_empty() {
        bridge::webview::ffi::qt_webview_navigate(native, &pending_url);
    }
        crate::qtrace!(
        "[tauri-runtime-qt] webview '{label}' created in window {window_native} (native {native}), url {}",
        pending_url
    );
    Ok(DetachedWebview {
        label: label.clone(),
        dispatcher: QtWebviewDispatcher { native, label },
    })
}

/// Creates a window, attaching its webview when tauri supplied one.
fn create_window_impl<T: UserEvent>(
    pending: PendingWindow<T, QtRuntime>,
) -> Result<DetachedWindow<T, QtRuntime>> {
    let label = pending.label.clone();
    let (id, dispatcher) = build_window::<T>(label.clone(), pending.window_builder)?;

    let webview = match pending.webview {
        Some(pending_webview) => {
            let webview = build_webview::<T>(dispatcher.native, pending_webview)?;
            Some(rt::window::DetachedWindowWebview {
                webview,
                use_https_scheme: false,
            })
        }
        None => None,
    };

    Ok(DetachedWindow {
        id,
        label,
        dispatcher,
        webview,
    })
}

/// Native handle of a window previously created by this runtime.
fn window_native_of(id: WindowId) -> Result<u64> {
    NATIVE_WINDOWS
        .lock()
        .iter()
        .find(|(window_id, _)| *window_id == id)
        .map(|(_, native)| *native)
        .ok_or(Error::WindowNotFound)
}

/// Window attributes builder: records the values the Qt window is created with.
#[derive(Debug, Clone)]
pub struct QtWindowBuilder {
    theme: Option<Theme>,
    has_icon: bool,
    title: String,
    width: f64,
    height: f64,
    visible: bool,
    decorations: bool,
    maximized: bool,
    center: bool,
    pub(crate) position: Option<(f64, f64)>,
}

impl Default for QtWindowBuilder {
    fn default() -> Self {
        Self {
            theme: None,
            has_icon: false,
            title: String::new(),
            width: 800.0,
            height: 600.0,
            visible: true,
            decorations: true,
            maximized: false,
            center: false,
            position: None,
        }
    }
}

impl rt::window::WindowBuilderBase for QtWindowBuilder {}

impl rt::window::WindowBuilder for QtWindowBuilder {
    fn new() -> Self {
        Self::default()
    }

    fn with_config(config: &WindowConfig) -> Self {
        Self {
            theme: config.theme.map(Theme::from),
            has_icon: false,
            title: config.title.clone(),
            width: config.width,
            height: config.height,
            visible: config.visible,
            decorations: config.decorations,
            maximized: config.maximized,
            center: config.center,
            position: None,
        }
    }

    fn center(self) -> Self {
        Self {
            center: true,
            ..self
        }
    }

    fn position(self, x: f64, y: f64) -> Self {
        Self {
            position: Some((x, y)),
            ..self
        }
    }

    fn inner_size(self, width: f64, height: f64) -> Self {
        Self {
            width,
            height,
            ..self
        }
    }

    fn min_inner_size(self, _min_width: f64, _min_height: f64) -> Self {
        self
    }

    fn max_inner_size(self, _max_width: f64, _max_height: f64) -> Self {
        self
    }

    fn inner_size_constraints(
        self,
        _constraints: WindowSizeConstraints,
    ) -> Self {
        self
    }

    fn prevent_overflow(self) -> Self {
        self
    }

    fn prevent_overflow_with_margin(self, _margin: rt::dpi::Size) -> Self {
        self
    }

    fn resizable(self, _resizable: bool) -> Self {
        self
    }

    fn maximizable(self, _maximizable: bool) -> Self {
        self
    }

    fn minimizable(self, _minimizable: bool) -> Self {
        self
    }

    fn closable(self, _closable: bool) -> Self {
        self
    }

    fn title<S: Into<String>>(self, title: S) -> Self {
        Self {
            title: title.into(),
            ..self
        }
    }

    fn fullscreen(self, _fullscreen: bool) -> Self {
        self
    }

    fn focused(self, _focused: bool) -> Self {
        self
    }

    fn focusable(self, _focusable: bool) -> Self {
        self
    }

    fn maximized(self, maximized: bool) -> Self {
        Self { maximized, ..self }
    }

    fn visible(self, visible: bool) -> Self {
        Self { visible, ..self }
    }

    fn transparent(self, _transparent: bool) -> Self {
        self
    }

    fn decorations(self, decorations: bool) -> Self {
        Self { decorations, ..self }
    }

    fn always_on_bottom(self, _always_on_bottom: bool) -> Self {
        self
    }

    fn always_on_top(self, _always_on_top: bool) -> Self {
        self
    }

    fn visible_on_all_workspaces(self, _visible_on_all_workspaces: bool) -> Self {
        self
    }

    fn content_protected(self, _protected: bool) -> Self {
        self
    }

    fn icon(self, _icon: Icon) -> crate::Result<Self> {
        Ok(self)
    }

    fn skip_taskbar(self, _skip: bool) -> Self {
        self
    }

    fn background_color(self, _color: Color) -> Self {
        self
    }

    fn shadow(self, _enable: bool) -> Self {
        self
    }

    #[cfg(target_os = "linux")]
    fn transient_for(self, _parent: &impl gtk::glib::IsA<gtk::Window>) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    fn title_bar_style(self, _style: tauri_utils::TitleBarStyle) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    fn traffic_light_position<P: Into<rt::dpi::Position>>(self, _position: P) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    fn hidden_title(self, _hidden: bool) -> Self {
        self
    }

    #[cfg(target_os = "macos")]
    fn tabbing_identifier(self, _identifier: &str) -> Self {
        self
    }

    fn theme(self, theme: Option<Theme>) -> Self {
        Self { theme, ..self }
    }

    fn has_icon(&self) -> bool {
        self.has_icon
    }

    fn get_theme(&self) -> Option<Theme> {
        self.theme
    }

    fn window_classname<S: Into<String>>(self, _window_classname: S) -> Self {
        self
    }

    fn no_redirection_bitmap(self, _enable: bool) -> Self {
        self
    }

    #[cfg(target_os = "android")]
    fn activity_name<S: Into<String>>(self, _class_name: S) -> Self {
        self
    }

    #[cfg(target_os = "android")]
    fn created_by_activity_name<S: Into<String>>(self, _class_name: S) -> Self {
        self
    }

    #[cfg(target_os = "ios")]
    fn requested_by_scene_identifier<S: Into<String>>(self, _identifier: S) -> Self {
        self
    }
}

impl<T: UserEvent> EventLoopProxy<T> for QtEventLoopProxy {
    fn send_event(&self, event: T) -> Result<()> {
        ops::runtime::proxy_send_event(event)
    }
}

impl<T: UserEvent> rt::RuntimeHandle<T> for QtRuntimeHandle {
    type Runtime = QtRuntime;

    fn create_proxy(&self) -> <Self::Runtime as rt::Runtime<T>>::EventLoopProxy {
        QtEventLoopProxy
    }

    fn request_exit(&self, code: i32) -> Result<()> {
        ops::runtime::request_exit(code)
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &self,
        pending: PendingWindow<T, Self::Runtime>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self::Runtime>> {
        create_window_impl::<T>(pending)
    }

    fn create_webview(
        &self,
        window_id: WindowId,
        pending: PendingWebview<T, Self::Runtime>,
    ) -> Result<DetachedWebview<T, Self::Runtime>> {
        build_webview::<T>(window_native_of(window_id)?, pending)
    }

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, _f: F) -> Result<()> {
        Ok(())
    }

    fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, HandleError> {
        ops::runtime::display_handle()
    }

    fn primary_monitor(&self) -> Result<Option<Monitor>> {
        ops::screen::primary_monitor()
    }

    fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
        ops::screen::monitor_from_point(x, y)
    }

    fn available_monitors(&self) -> Result<Vec<Monitor>> {
        ops::screen::available_monitors()
    }

    fn cursor_position(&self) -> Result<rt::dpi::PhysicalPosition<f64>> {
        ops::screen::cursor_position()
    }

    fn set_theme(&self, _theme: Option<Theme>) {}

    fn set_device_event_filter(&self, _filter: DeviceEventFilter) {}
}

impl<T: UserEvent> rt::Runtime<T> for QtRuntime {
    type WindowDispatcher = QtWindowDispatcher;
    type WebviewDispatcher = QtWebviewDispatcher;
    type Handle = QtRuntimeHandle;
    type EventLoopProxy = QtEventLoopProxy;

    fn new(_args: RuntimeInitArgs) -> Result<Self> {
        // Qt requires the QApplication before any GUI object; schemes must be
        // registered in the same window of opportunity (see PLAN.md M1).
        if qt::ffi::qt_init(0) != 0 {
            return Err(Error::CreateWindow);
        }
        Ok(Self)
    }

    fn new_any_thread(args: RuntimeInitArgs) -> Result<Self> {
        // tauri core constructs the runtime through this entry point on Linux;
        // Qt still requires the QApplication on the process' main thread, which
        // is where tauri calls it from.
        <Self as rt::Runtime<T>>::new(args)
    }

    fn create_proxy(&self) -> Self::EventLoopProxy {
        QtEventLoopProxy
    }

    fn handle(&self) -> Self::Handle {
        QtRuntimeHandle
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &self,
        pending: PendingWindow<T, Self>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self>> {
        create_window_impl::<T>(pending)
    }

    fn create_webview(
        &self,
        window_id: WindowId,
        pending: PendingWebview<T, Self>,
    ) -> Result<DetachedWebview<T, Self>> {
        build_webview::<T>(window_native_of(window_id)?, pending)
    }

    fn primary_monitor(&self) -> Option<Monitor> {
        ops::screen::primary_monitor().ok().flatten()
    }

    fn monitor_from_point(&self, x: f64, y: f64) -> Option<Monitor> {
        ops::screen::monitor_from_point(x, y).ok().flatten()
    }

    fn available_monitors(&self) -> Vec<Monitor> {
        ops::screen::available_monitors().unwrap_or_default()
    }

    fn cursor_position(&self) -> Result<rt::dpi::PhysicalPosition<f64>> {
        ops::screen::cursor_position()
    }

    fn set_theme(&self, _theme: Option<Theme>) {}

    fn set_device_event_filter(&mut self, _filter: DeviceEventFilter) {}

    fn run_iteration<F: FnMut(RunEvent<T>) + 'static>(&mut self, _callback: F) {
        // Qt owns the thread inside exec(); the pump delivers user events.
        ops::runtime::run_iteration();
    }

    fn run_return<F: FnMut(RunEvent<T>) + 'static>(self, mut callback: F) -> i32 {
        callback(RunEvent::Ready);
        // Qt owns the thread inside exec(), so user events are delivered from
        // the pump tick. The run callback is not required to be `Send`, hence
        // the main-thread-only sink wrapper.
        let callback = std::rc::Rc::new(std::cell::RefCell::new(callback));
        let user_callback = std::rc::Rc::clone(&callback);
        ops::runtime::set_event_sink(ops::runtime::main_thread_sink(move |event| {
            if let Ok(event) = event.downcast::<T>() {
                user_callback.borrow_mut()(RunEvent::UserEvent(*event));
            }
        }));
        ops::runtime::start_pump();
        let code = qt::ffi::qt_run();
        callback.borrow_mut()(RunEvent::Exit);
        code
    }

    fn run<F: FnMut(RunEvent<T>) + 'static>(self, mut callback: F) {
                crate::qtrace!("[tauri-runtime-qt] run: emitting Ready");
        callback(RunEvent::Ready);
        let callback = std::rc::Rc::new(std::cell::RefCell::new(callback));
        let user_callback = std::rc::Rc::clone(&callback);
        ops::runtime::set_event_sink(ops::runtime::main_thread_sink(move |event| {
            if let Ok(event) = event.downcast::<T>() {
                user_callback.borrow_mut()(RunEvent::UserEvent(*event));
            }
        }));
        ops::runtime::start_pump();
                crate::qtrace!("[tauri-runtime-qt] run: entering Qt event loop");
        qt::ffi::qt_run();
        callback.borrow_mut()(RunEvent::Exit);
    }
}

impl<T: UserEvent> rt::WebviewDispatch<T> for QtWebviewDispatcher {
    type Runtime = QtRuntime;

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
        // Qt runs everything on the GUI thread; callers reach us through the
        // event loop, so executing inline is correct for the MVP.
        f();
        Ok(())
    }

    fn on_webview_event<F: Fn(&WebviewEvent) + Send + 'static>(&self, f: F) -> WebviewEventId {
        register_webview_handler(self.native, f)
    }

    fn with_webview<F: FnOnce(Box<dyn std::any::Any>) + Send + 'static>(&self, f: F) -> Result<()> {
        // M2: hand out the real QWebEngineView pointer once the C++ side can
        // vend it as an Any-safe value.
        f(Box::new(self.native));
        Ok(())
    }

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn open_devtools(&self) {
        ops::webview::open_devtools(self.native);
    }

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn close_devtools(&self) {
        ops::webview::close_devtools(self.native);
    }

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn is_devtools_open(&self) -> Result<bool> {
        ops::webview::is_devtools_open(self.native)
    }

    fn url(&self) -> Result<String> {
        ops::webview::url(self.native)
    }

    fn bounds(&self) -> Result<rt::dpi::Rect> {
        ops::webview::bounds(self.native)
    }

    fn position(&self) -> Result<rt::dpi::PhysicalPosition<i32>> {
        ops::webview::position(self.native)
    }

    fn size(&self) -> Result<rt::dpi::PhysicalSize<u32>> {
        ops::webview::size(self.native)
    }

    fn navigate(&self, url: url::Url) -> Result<()> {
        ops::webview::navigate(self.native, url.as_str())
    }

    fn reload(&self) -> Result<()> {
        ops::webview::reload(self.native)
    }

    fn print(&self) -> Result<()> {
        ops::webview::print(self.native)
    }

    fn close(&self) -> Result<()> {
        ops::webview::close(self.native)
    }

    fn set_bounds(&self, bounds: rt::dpi::Rect) -> Result<()> {
        ops::webview::set_bounds(self.native, bounds)
    }

    fn set_size(&self, size: rt::dpi::Size) -> Result<()> {
        ops::webview::set_size(self.native, size)
    }

    fn set_position(&self, position: rt::dpi::Position) -> Result<()> {
        ops::webview::set_position(self.native, position)
    }

    fn set_focus(&self) -> Result<()> {
        ops::webview::set_focus(self.native)
    }

    fn hide(&self) -> Result<()> {
        ops::webview::hide(self.native)
    }

    fn show(&self) -> Result<()> {
        ops::webview::show(self.native)
    }

    fn eval_script<S: Into<String>>(&self, script: S) -> Result<()> {
        ops::webview::eval(self.native, &script.into())
    }

    fn eval_script_with_callback<S: Into<String>>(
        &self,
        script: S,
        callback: impl Fn(String) + Send + 'static,
    ) -> Result<()> {
        ops::webview::eval_with_callback(self.native, &script.into(), callback)
    }

    fn reparent(&self, window_id: WindowId) -> Result<()> {
        ops::webview::reparent(self.native, window_native_of(window_id)?)
    }

    fn cookies_for_url(&self, url: url::Url) -> Result<Vec<Cookie<'static>>> {
        ops::cookies::cookies_for_url(self.native, &url)
    }

    fn cookies(&self) -> Result<Vec<Cookie<'static>>> {
        ops::cookies::cookies(self.native)
    }

    fn set_cookie(&self, cookie: cookie::Cookie<'_>) -> Result<()> {
        ops::cookies::set_cookie(self.native, cookie)
    }

    fn delete_cookie(&self, cookie: cookie::Cookie<'_>) -> Result<()> {
        ops::cookies::delete_cookie(self.native, cookie)
    }

    fn set_auto_resize(&self, _auto_resize: bool) -> Result<()> {
        // The view fills the window through Qt's layout; nothing to toggle yet.
        Ok(())
    }

    fn set_zoom(&self, scale_factor: f64) -> Result<()> {
        ops::webview::set_zoom(self.native, scale_factor)
    }

    fn set_background_color(&self, color: Option<Color>) -> Result<()> {
        ops::webview::set_background_color(self.native, color)
    }

    fn clear_all_browsing_data(&self) -> Result<()> {
        ops::cookies::clear_all_browsing_data(self.native)
    }
}

impl<T: UserEvent> rt::WindowDispatch<T> for QtWindowDispatcher {
    type Runtime = QtRuntime;
    type WindowBuilder = QtWindowBuilder;

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, f: F) -> Result<()> {
        f();
        Ok(())
    }

    fn on_window_event<F: Fn(&WindowEvent) + Send + 'static>(&self, f: F) -> WindowEventId {
        register_window_handler(self.native, f)
    }

    fn scale_factor(&self) -> Result<f64> {
        ops::window::scale_factor(self.native)
    }

    fn inner_position(&self) -> Result<rt::dpi::PhysicalPosition<i32>> {
        ops::window::inner_position(self.native)
    }

    fn outer_position(&self) -> Result<rt::dpi::PhysicalPosition<i32>> {
        ops::window::outer_position(self.native)
    }

    fn inner_size(&self) -> Result<rt::dpi::PhysicalSize<u32>> {
        ops::window::inner_size(self.native)
    }

    fn outer_size(&self) -> Result<rt::dpi::PhysicalSize<u32>> {
        ops::window::outer_size(self.native)
    }

    fn is_fullscreen(&self) -> Result<bool> {
        ops::window::is_fullscreen(self.native)
    }

    fn is_minimized(&self) -> Result<bool> {
        ops::window::is_minimized(self.native)
    }

    fn is_maximized(&self) -> Result<bool> {
        ops::window::is_maximized(self.native)
    }

    fn is_focused(&self) -> Result<bool> {
        ops::window::is_focused(self.native)
    }

    fn is_decorated(&self) -> Result<bool> {
        ops::window::is_decorated(self.native)
    }

    fn is_resizable(&self) -> Result<bool> {
        ops::window::is_resizable(self.native)
    }

    fn is_visible(&self) -> Result<bool> {
        ops::window::is_visible(self.native)
    }

    fn is_enabled(&self) -> Result<bool> {
        ops::window::is_enabled(self.native)
    }

    fn is_always_on_top(&self) -> Result<bool> {
        ops::window::is_always_on_top(self.native)
    }

    fn title(&self) -> Result<String> {
        ops::window::title(self.native)
    }

    fn theme(&self) -> Result<Theme> {
        ops::window::theme(self.native)
    }

    fn center(&self) -> Result<()> {
        ops::window::center(self.native)
    }

    fn minimize(&self) -> Result<()> {
        ops::window::minimize(self.native)
    }

    fn unminimize(&self) -> Result<()> {
        ops::window::unminimize(self.native)
    }

    fn show(&self) -> Result<()> {
        ops::window::show(self.native)
    }

    fn hide(&self) -> Result<()> {
        ops::window::hide(self.native)
    }

    fn close(&self) -> Result<()> {
        ops::window::close(self.native)
    }

    fn destroy(&self) -> Result<()> {
        ops::window::destroy(self.native)
    }

    fn set_focus(&self) -> Result<()> {
        ops::window::set_focus(self.native)
    }

    fn start_dragging(&self) -> Result<()> {
        ops::window::start_dragging(self.native)
    }

    fn is_maximizable(&self) -> Result<bool> {
        // The trait documents this as unsupported on Linux (no native maximize
        // button state), so this reports the capability, not a measured state.
        Ok(true)
    }

    fn is_minimizable(&self) -> Result<bool> {
        // Linux: unsupported by the trait's own documentation.
        Ok(true)
    }

    fn is_closable(&self) -> Result<bool> {
        // Linux: unsupported by the trait's own documentation.
        Ok(true)
    }

    fn current_monitor(&self) -> Result<Option<Monitor>> {
        ops::screen::monitor_for_window(self.native)
    }

    fn primary_monitor(&self) -> Result<Option<Monitor>> {
        ops::screen::primary_monitor()
    }

    fn monitor_from_point(&self, x: f64, y: f64) -> Result<Option<Monitor>> {
        ops::screen::monitor_from_point(x, y)
    }

    fn available_monitors(&self) -> Result<Vec<Monitor>> {
        ops::screen::available_monitors()
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    fn gtk_window(&self) -> Result<gtk::ApplicationWindow> {
        // No GTK window exists in a Qt runtime; use `window_handle`.
        Err(Error::CreateWindow)
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    fn default_vbox(&self) -> Result<gtk::Box> {
        Err(Error::CreateWindow)
    }

    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        ops::runtime::window_handle(self.native)
    }

    fn request_user_attention(&self, _request_type: Option<UserAttentionType>) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::request_user_attention"))
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &mut self,
        pending: PendingWindow<T, Self::Runtime>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self::Runtime>> {
        create_window_impl::<T>(pending)
    }

    fn create_webview(
        &mut self,
        pending: PendingWebview<T, Self::Runtime>,
    ) -> Result<DetachedWebview<T, Self::Runtime>> {
        build_webview::<T>(self.native, pending)
    }

    fn set_resizable(&self, resizable: bool) -> Result<()> {
        ops::window::set_resizable(self.native, resizable)
    }

    fn set_enabled(&self, enabled: bool) -> Result<()> {
        ops::window::set_enabled(self.native, enabled)
    }

    fn set_maximizable(&self, _maximizable: bool) -> Result<()> {
        // Linux: unsupported by the trait's own documentation.
        Ok(())
    }

    fn set_minimizable(&self, _minimizable: bool) -> Result<()> {
        // Linux: unsupported by the trait's own documentation.
        Ok(())
    }

    fn set_closable(&self, _closable: bool) -> Result<()> {
        // Linux: unsupported by the trait's own documentation.
        Ok(())
    }

    fn set_title<S: Into<String>>(&self, title: S) -> Result<()> {
        ops::window::set_title(self.native, &title.into())
    }

    fn maximize(&self) -> Result<()> {
        ops::window::set_maximized(self.native, true)
    }

    fn unmaximize(&self) -> Result<()> {
        ops::window::set_maximized(self.native, false)
    }

    fn set_decorations(&self, decorations: bool) -> Result<()> {
        ops::window::set_decorations(self.native, decorations)
    }

    fn set_shadow(&self, _enable: bool) -> Result<()> {
        // Linux: unsupported by the trait's own documentation.
        Ok(())
    }

    fn set_always_on_bottom(&self, always_on_bottom: bool) -> Result<()> {
        ops::window::set_always_on_bottom(self.native, always_on_bottom)
    }

    fn set_always_on_top(&self, always_on_top: bool) -> Result<()> {
        ops::window::set_always_on_top(self.native, always_on_top)
    }

    fn set_visible_on_all_workspaces(&self, _visible_on_all_workspaces: bool) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_visible_on_all_workspaces"))
    }

    fn set_background_color(&self, color: Option<Color>) -> Result<()> {
        ops::window::set_background_color(self.native, color)
    }

    fn set_content_protected(&self, _protected: bool) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_content_protected"))
    }

    fn set_size(&self, size: rt::dpi::Size) -> Result<()> {
        let (w, h) = match size {
            rt::dpi::Size::Physical(s) => (s.width as i32, s.height as i32),
            rt::dpi::Size::Logical(s) => (s.width as i32, s.height as i32),
        };
        ops::window::set_size(self.native, w, h)
    }

    fn set_min_size(&self, size: Option<rt::dpi::Size>) -> Result<()> {
        ops::window::set_min_size(self.native, size)
    }

    fn set_max_size(&self, size: Option<rt::dpi::Size>) -> Result<()> {
        ops::window::set_max_size(self.native, size)
    }

    fn set_size_constraints(&self, constraints: WindowSizeConstraints) -> Result<()> {
        ops::window::set_size_constraints(self.native, constraints)
    }

    fn set_position(&self, position: rt::dpi::Position) -> Result<()> {
        ops::window::set_position(self.native, position)
    }

    fn set_fullscreen(&self, fullscreen: bool) -> Result<()> {
        ops::window::set_fullscreen(self.native, fullscreen)
    }

    fn set_fullscreen_on_monitor(&self, _position: rt::dpi::PhysicalPosition<f64>) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_fullscreen_on_monitor"))
    }

    fn set_focusable(&self, _focusable: bool) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_focusable"))
    }

    fn set_icon(&self, icon: Icon) -> Result<()> {
        ops::window::set_icon(self.native, icon)
    }

    fn set_skip_taskbar(&self, _skip: bool) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_skip_taskbar"))
    }

    fn set_cursor_grab(&self, _grab: bool) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_cursor_grab"))
    }

    fn set_cursor_visible(&self, _visible: bool) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_cursor_visible"))
    }

    fn set_cursor_icon(&self, _icon: CursorIcon) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_cursor_icon"))
    }

    fn set_cursor_position<Pos: Into<rt::dpi::Position>>(&self, _position: Pos) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_cursor_position"))
    }

    fn set_ignore_cursor_events(&self, _ignore: bool) -> Result<()> {
        Err(ops::unimplemented("WindowDispatch::set_ignore_cursor_events"))
    }

    fn start_resize_dragging(&self, direction: ResizeDirection) -> Result<()> {
        let direction = match direction {
            ResizeDirection::East => 1,
            ResizeDirection::North => 2,
            ResizeDirection::NorthEast => 3,
            ResizeDirection::NorthWest => 4,
            ResizeDirection::South => 5,
            ResizeDirection::SouthEast => 6,
            ResizeDirection::SouthWest => 7,
            ResizeDirection::West => 8,
        };
        ops::window::start_resize_dragging(self.native, direction)
    }

    fn set_badge_count(&self, _count: Option<i64>, _desktop_filename: Option<String>) -> Result<()> {
        // Linux: unsupported by the trait's own documentation.
        Ok(())
    }

    fn set_badge_label(&self, _label: Option<String>) -> Result<()> {
        // macOS-only per the trait documentation.
        Ok(())
    }

    fn set_overlay_icon(&self, _icon: Option<Icon>) -> Result<()> {
        // Windows-only per the trait documentation.
        Ok(())
    }

    fn set_progress_bar(&self, _progress_state: ProgressBarState) -> Result<()> {
        // Needs libunity on Linux; unsupported by the trait documentation.
        Ok(())
    }

    fn set_title_bar_style(&self, _style: tauri_utils::TitleBarStyle) -> Result<()> {
        // macOS-only per the trait documentation.
        Ok(())
    }

    fn set_traffic_light_position(&self, _position: rt::dpi::Position) -> Result<()> {
        // macOS-only per the trait documentation.
        Ok(())
    }

    fn set_theme(&self, _theme: Option<Theme>) -> Result<()> {
        // Linux: app-wide; handled by `Runtime::set_theme`.
        Ok(())
    }
}

/// Placeholder for webview platform handle exposure (`with_webview`).
#[derive(Debug)]
pub struct QtWebviewHandle;
