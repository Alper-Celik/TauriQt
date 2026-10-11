// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! No-op skeleton of a Qt-based runtime for Tauri (PLAN.md M4, step 1).
//!
//! All trait methods are implemented with `todo!()` placeholders or functional
//! no-ops so the crate compiles against the full `tauri-runtime` surface.
//! Qt mappings for each method are documented in RUNTIME-TRAITS.md.

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

pub mod qt;

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

/// `uri_scheme_protocols` registered per native webview handle (scheme name → handler).
static SCHEME_PROTOCOLS: Mutex<Vec<(u64, HashMap<String, SchemeHandler>)>> =
    Mutex::new(Vec::new());

/// Creates the Qt window for a pending window and records its handle.
fn build_window<T: UserEvent>(
    label: String,
    builder: QtWindowBuilder,
) -> Result<(WindowId, QtWindowDispatcher)> {
    let native = qt::ffi::qt_window_create(
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
        qt::ffi::qt_window_set_maximized(native, true);
    }
    if builder.center {
        qt::ffi::qt_window_center(native);
    }
    if let Some((x, y)) = builder.position {
        let _ = (x, y); // M4: absolute positioning is a no-op on Wayland.
    }
    let id = WindowId::from(native as u32);
    NATIVE_WINDOWS.lock().push((id, native));
    Ok((id, QtWindowDispatcher { native }))
}

/// Creates the Qt webview inside an existing window and installs the schemes.
fn build_webview<T: UserEvent>(
    window_native: u64,
    pending: PendingWebview<T, QtRuntime>,
) -> Result<DetachedWebview<T, QtRuntime>> {
    let label = pending.label.clone();
    let native = qt::ffi::qt_webview_create(window_native, &pending.url);
    if native == 0 {
        return Err(Error::CreateWebview(
            "qt_webview_create returned a null handle".into(),
        ));
    }
    qt::ffi::qt_window_attach_webview(window_native, qt::ffi::qt_webview_native_handle(native));
    SCHEME_PROTOCOLS
        .lock()
        .push((native, pending.uri_scheme_protocols));
    qt::ffi::qt_scheme_install_handler(native);
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
    fn send_event(&self, _event: T) -> Result<()> {
        // M4: push into queue + wake Qt main loop.
        Err(Error::FailedToSendMessage)
    }
}

impl<T: UserEvent> rt::RuntimeHandle<T> for QtRuntimeHandle {
    type Runtime = QtRuntime;

    fn create_proxy(&self) -> <Self::Runtime as rt::Runtime<T>>::EventLoopProxy {
        QtEventLoopProxy
    }

    fn request_exit(&self, _code: i32) -> Result<()> {
        Err(Error::EventLoopClosed)
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
        Err(HandleError::Unavailable)
    }

    fn primary_monitor(&self) -> Result<Option<Monitor>> {
        Ok(None)
    }

    fn monitor_from_point(&self, _x: f64, _y: f64) -> Result<Option<Monitor>> {
        Ok(None)
    }

    fn available_monitors(&self) -> Result<Vec<Monitor>> {
        Ok(Vec::new())
    }

    fn cursor_position(&self) -> Result<rt::dpi::PhysicalPosition<f64>> {
        Err(Error::FailedToGetCursorPosition)
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
        None
    }

    fn monitor_from_point(&self, _x: f64, _y: f64) -> Option<Monitor> {
        None
    }

    fn available_monitors(&self) -> Vec<Monitor> {
        Vec::new()
    }

    fn cursor_position(&self) -> Result<rt::dpi::PhysicalPosition<f64>> {
        Err(Error::FailedToGetCursorPosition)
    }

    fn set_theme(&self, _theme: Option<Theme>) {}

    fn set_device_event_filter(&mut self, _filter: DeviceEventFilter) {}

    fn run_iteration<F: FnMut(RunEvent<T>) + 'static>(&mut self, _callback: F) {
        // M4: QCoreApplication::processEvents()
    }

    fn run_return<F: FnMut(RunEvent<T>) + 'static>(self, mut callback: F) -> i32 {
        callback(RunEvent::Ready);
        let code = qt::ffi::qt_run();
        callback(RunEvent::Exit);
        code
    }

    fn run<F: FnMut(RunEvent<T>) + 'static>(self, mut callback: F) {
        callback(RunEvent::Ready);
        qt::ffi::qt_run();
        callback(RunEvent::Exit);
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
        qt::ffi::qt_webview_open_devtools(self.native);
    }

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn close_devtools(&self) {
        qt::ffi::qt_webview_close_devtools(self.native);
    }

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn is_devtools_open(&self) -> Result<bool> {
        Ok(qt::ffi::qt_webview_is_devtools_open(self.native))
    }

    fn url(&self) -> Result<String> {
        Ok(qt::ffi::qt_webview_url(self.native))
    }

    fn bounds(&self) -> Result<rt::dpi::Rect> {
        // Geometry is layout-managed by Qt; report the origin for the MVP.
        Ok(rt::dpi::Rect {
            position: rt::dpi::Position::Physical(rt::dpi::PhysicalPosition::new(0, 0)),
            size: rt::dpi::Size::Physical(rt::dpi::PhysicalSize::new(0, 0)),
        })
    }

    fn position(&self) -> Result<rt::dpi::PhysicalPosition<i32>> {
        Ok(rt::dpi::PhysicalPosition::new(0, 0))
    }

    fn size(&self) -> Result<rt::dpi::PhysicalSize<u32>> {
        Ok(rt::dpi::PhysicalSize::new(0, 0))
    }

    fn navigate(&self, url: url::Url) -> Result<()> {
        qt::ffi::qt_webview_navigate(self.native, url.as_str());
        Ok(())
    }

    fn reload(&self) -> Result<()> {
        qt::ffi::qt_webview_reload(self.native);
        Ok(())
    }

    fn print(&self) -> Result<()> {
        qt::ffi::qt_webview_print(self.native);
        Ok(())
    }

    fn close(&self) -> Result<()> {
        qt::ffi::qt_webview_close(self.native);
        Ok(())
    }

    fn set_bounds(&self, bounds: rt::dpi::Rect) -> Result<()> {
        let (x, y) = match bounds.position {
            rt::dpi::Position::Physical(p) => (p.x, p.y),
            rt::dpi::Position::Logical(p) => (p.x as i32, p.y as i32),
        };
        let (w, h) = match bounds.size {
            rt::dpi::Size::Physical(s) => (s.width as i32, s.height as i32),
            rt::dpi::Size::Logical(s) => (s.width as i32, s.height as i32),
        };
        qt::ffi::qt_webview_set_bounds(self.native, x, y, w, h);
        Ok(())
    }

    fn set_size(&self, _size: rt::dpi::Size) -> Result<()> {
        // MVP: the view fills the window; Qt's layout owns the geometry.
        Ok(())
    }

    fn set_position(&self, _position: rt::dpi::Position) -> Result<()> {
        Ok(())
    }

    fn set_focus(&self) -> Result<()> {
        Ok(())
    }

    fn hide(&self) -> Result<()> {
        qt::ffi::qt_webview_hide(self.native);
        Ok(())
    }

    fn show(&self) -> Result<()> {
        qt::ffi::qt_webview_show(self.native);
        Ok(())
    }

    fn eval_script<S: Into<String>>(&self, script: S) -> Result<()> {
        qt::ffi::qt_webview_eval(self.native, &script.into());
        Ok(())
    }

    fn eval_script_with_callback<S: Into<String>>(
        &self,
        script: S,
        callback: impl Fn(String) + Send + 'static,
    ) -> Result<()> {
        // M1: needs a Rust-callback bridge over QWebEnginePage::runJavaScript.
        let _ = (script.into(), callback);
        Err(Error::CreateWebview(
            "eval_script_with_callback is not implemented yet".into(),
        ))
    }

    fn reparent(&self, _window_id: WindowId) -> Result<()> {
        Err(Error::CreateWebview(
            "reparent is not implemented yet".into(),
        ))
    }

    fn cookies_for_url(&self, _url: url::Url) -> Result<Vec<Cookie<'static>>> {
        Err(Error::CreateWebview(
            "cookies need an async QWebEngineCookieStore round-trip".into(),
        ))
    }

    fn cookies(&self) -> Result<Vec<Cookie<'static>>> {
        Err(Error::CreateWebview(
            "cookies need an async QWebEngineCookieStore round-trip".into(),
        ))
    }

    fn set_cookie(&self, _cookie: cookie::Cookie<'_>) -> Result<()> {
        Err(Error::CreateWebview(
            "set_cookie is not implemented yet".into(),
        ))
    }

    fn delete_cookie(&self, _cookie: cookie::Cookie<'_>) -> Result<()> {
        Err(Error::CreateWebview(
            "delete_cookie is not implemented yet".into(),
        ))
    }

    fn set_auto_resize(&self, _auto_resize: bool) -> Result<()> {
        Ok(())
    }

    fn set_zoom(&self, scale_factor: f64) -> Result<()> {
        qt::ffi::qt_webview_set_zoom(self.native, scale_factor);
        Ok(())
    }

    fn set_background_color(&self, color: Option<Color>) -> Result<()> {
        if let Some(Color(r, g, b, a)) = color {
            qt::ffi::qt_webview_set_background_color(
                self.native,
                r as i32,
                g as i32,
                b as i32,
                a as i32,
            );
        }
        Ok(())
    }

    fn clear_all_browsing_data(&self) -> Result<()> {
        Err(Error::CreateWebview(
            "clear_all_browsing_data is not implemented yet".into(),
        ))
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
        Ok(qt::ffi::qt_window_scale_factor(self.native))
    }

    fn inner_position(&self) -> Result<rt::dpi::PhysicalPosition<i32>> {
        Ok(rt::dpi::PhysicalPosition::new(0, 0))
    }

    fn outer_position(&self) -> Result<rt::dpi::PhysicalPosition<i32>> {
        Ok(rt::dpi::PhysicalPosition::new(0, 0))
    }

    fn inner_size(&self) -> Result<rt::dpi::PhysicalSize<u32>> {
        Ok(rt::dpi::PhysicalSize::new(0, 0))
    }

    fn outer_size(&self) -> Result<rt::dpi::PhysicalSize<u32>> {
        Ok(rt::dpi::PhysicalSize::new(0, 0))
    }

    fn is_fullscreen(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_minimized(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_maximized(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_focused(&self) -> Result<bool> {
        Ok(false)
    }

    fn is_decorated(&self) -> Result<bool> {
        Ok(true)
    }

    fn is_resizable(&self) -> Result<bool> {
        Ok(true)
    }

    fn is_maximizable(&self) -> Result<bool> {
        Ok(true)
    }

    fn is_minimizable(&self) -> Result<bool> {
        Ok(true)
    }

    fn is_closable(&self) -> Result<bool> {
        Ok(true)
    }

    fn is_visible(&self) -> Result<bool> {
        Ok(qt::ffi::qt_window_is_visible(self.native))
    }

    fn is_enabled(&self) -> Result<bool> {
        Ok(true)
    }

    fn is_always_on_top(&self) -> Result<bool> {
        Ok(false)
    }

    fn title(&self) -> Result<String> {
        Ok(String::new())
    }

    fn current_monitor(&self) -> Result<Option<Monitor>> {
        Ok(None)
    }

    fn primary_monitor(&self) -> Result<Option<Monitor>> {
        Ok(None)
    }

    fn monitor_from_point(&self, _x: f64, _y: f64) -> Result<Option<Monitor>> {
        Ok(None)
    }

    fn available_monitors(&self) -> Result<Vec<Monitor>> {
        Ok(Vec::new())
    }

    #[cfg(any(
        target_os = "linux",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    fn gtk_window(&self) -> Result<gtk::ApplicationWindow> {
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
        // M2: X11 (QX11Application) / Wayland (QWaylandApplication) display handle.
        Err(HandleError::Unavailable)
    }

    fn theme(&self) -> Result<Theme> {
        Ok(Theme::Light)
    }

    fn center(&self) -> Result<()> {
        qt::ffi::qt_window_center(self.native);
        Ok(())
    }

    fn request_user_attention(&self, _request_type: Option<UserAttentionType>) -> Result<()> {
        Ok(())
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

    fn set_resizable(&self, _resizable: bool) -> Result<()> {
        Ok(())
    }

    fn set_enabled(&self, _enabled: bool) -> Result<()> {
        Ok(())
    }

    fn set_maximizable(&self, _maximizable: bool) -> Result<()> {
        Ok(())
    }

    fn set_minimizable(&self, _minimizable: bool) -> Result<()> {
        Ok(())
    }

    fn set_closable(&self, _closable: bool) -> Result<()> {
        Ok(())
    }

    fn set_title<S: Into<String>>(&self, title: S) -> Result<()> {
        qt::ffi::qt_window_set_title(self.native, &title.into());
        Ok(())
    }

    fn maximize(&self) -> Result<()> {
        qt::ffi::qt_window_set_maximized(self.native, true);
        Ok(())
    }

    fn unmaximize(&self) -> Result<()> {
        qt::ffi::qt_window_set_maximized(self.native, false);
        Ok(())
    }

    fn minimize(&self) -> Result<()> {
        Ok(())
    }

    fn unminimize(&self) -> Result<()> {
        Ok(())
    }

    fn show(&self) -> Result<()> {
        qt::ffi::qt_window_show(self.native);
        Ok(())
    }

    fn hide(&self) -> Result<()> {
        qt::ffi::qt_window_hide(self.native);
        Ok(())
    }

    fn close(&self) -> Result<()> {
        qt::ffi::qt_window_close(self.native);
        Ok(())
    }

    fn destroy(&self) -> Result<()> {
        qt::ffi::qt_window_close(self.native);
        Ok(())
    }

    fn set_decorations(&self, decorations: bool) -> Result<()> {
        qt::ffi::qt_window_set_decorations(self.native, decorations);
        Ok(())
    }

    fn set_shadow(&self, _enable: bool) -> Result<()> {
        Ok(())
    }

    fn set_always_on_bottom(&self, _always_on_bottom: bool) -> Result<()> {
        Ok(())
    }

    fn set_always_on_top(&self, _always_on_top: bool) -> Result<()> {
        Ok(())
    }

    fn set_visible_on_all_workspaces(&self, _visible_on_all_workspaces: bool) -> Result<()> {
        Ok(())
    }

    fn set_background_color(&self, _color: Option<Color>) -> Result<()> {
        Ok(())
    }

    fn set_content_protected(&self, _protected: bool) -> Result<()> {
        Ok(())
    }

    fn set_size(&self, size: rt::dpi::Size) -> Result<()> {
        let (w, h) = match size {
            rt::dpi::Size::Physical(s) => (s.width as i32, s.height as i32),
            rt::dpi::Size::Logical(s) => (s.width as i32, s.height as i32),
        };
        qt::ffi::qt_window_set_size(self.native, w, h);
        Ok(())
    }

    fn set_min_size(&self, _size: Option<rt::dpi::Size>) -> Result<()> {
        Ok(())
    }

    fn set_max_size(&self, _size: Option<rt::dpi::Size>) -> Result<()> {
        Ok(())
    }

    fn set_size_constraints(&self, _constraints: WindowSizeConstraints) -> Result<()> {
        Ok(())
    }

    fn set_position(&self, _position: rt::dpi::Position) -> Result<()> {
        // Wayland does not allow absolute positioning.
        Ok(())
    }

    fn set_fullscreen(&self, _fullscreen: bool) -> Result<()> {
        Ok(())
    }

    fn set_fullscreen_on_monitor(&self, _position: rt::dpi::PhysicalPosition<f64>) -> Result<()> {
        Ok(())
    }

    fn set_focus(&self) -> Result<()> {
        Ok(())
    }

    fn set_focusable(&self, _focusable: bool) -> Result<()> {
        Ok(())
    }

    fn set_icon(&self, _icon: Icon) -> Result<()> {
        Ok(())
    }

    fn set_skip_taskbar(&self, _skip: bool) -> Result<()> {
        Ok(())
    }

    fn set_cursor_grab(&self, _grab: bool) -> Result<()> {
        Ok(())
    }

    fn set_cursor_visible(&self, _visible: bool) -> Result<()> {
        Ok(())
    }

    fn set_cursor_icon(&self, _icon: CursorIcon) -> Result<()> {
        Ok(())
    }

    fn set_cursor_position<Pos: Into<rt::dpi::Position>>(&self, _position: Pos) -> Result<()> {
        Ok(())
    }

    fn set_ignore_cursor_events(&self, _ignore: bool) -> Result<()> {
        Ok(())
    }

    fn start_dragging(&self) -> Result<()> {
        Ok(())
    }

    fn start_resize_dragging(&self, _direction: ResizeDirection) -> Result<()> {
        Ok(())
    }

    fn set_badge_count(&self, _count: Option<i64>, _desktop_filename: Option<String>) -> Result<()> {
        Ok(())
    }

    fn set_badge_label(&self, _label: Option<String>) -> Result<()> {
        Ok(())
    }

    fn set_overlay_icon(&self, _icon: Option<Icon>) -> Result<()> {
        Ok(())
    }

    fn set_progress_bar(&self, _progress_state: ProgressBarState) -> Result<()> {
        Ok(())
    }

    fn set_title_bar_style(&self, _style: tauri_utils::TitleBarStyle) -> Result<()> {
        Ok(())
    }

    fn set_traffic_light_position(&self, _position: rt::dpi::Position) -> Result<()> {
        Ok(())
    }

    fn set_theme(&self, _theme: Option<Theme>) -> Result<()> {
        Ok(())
    }
}

/// Placeholder for webview platform handle exposure (`with_webview`).
#[derive(Debug)]
pub struct QtWebviewHandle;
