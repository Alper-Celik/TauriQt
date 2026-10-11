// SPDX-FileCopyrightText: 2026 Alper Çelik
// SPDX-License-Identifier: MIT

//! No-op skeleton of a Qt-based runtime for Tauri (PLAN.md M4, step 1).
//!
//! All trait methods are implemented with `todo!()` placeholders or functional
//! no-ops so the crate compiles against the full `tauri-runtime` surface.
//! Qt mappings for each method are documented in RUNTIME-TRAITS.md.

use std::borrow::Cow;

use raw_window_handle::{DisplayHandle, HandleError, WindowHandle};
use tauri_runtime as rt;
use tauri_runtime::{
    monitor::Monitor,
    webview::{DetachedWebview, PendingWebview},
    window::{
        CursorIcon, DetachedWindow, PendingWindow, RawWindow, WindowBuilderBase, WindowEvent,
        WindowId, WindowSizeConstraints, WebviewEvent,
    },
    Cookie, DeviceEventFilter, Error, EventLoopProxy, Icon, ProgressBarState, ResizeDirection,
    Result, RunEvent, RuntimeInitArgs, UserAttentionType, UserEvent, WebviewEventId, WindowEventId,
};
use tauri_utils::config::{Color, WindowConfig};
use tauri_utils::Theme;

/// The Qt runtime. Constructed on the main thread; owns the `QApplication`.
#[derive(Debug, Clone, Copy, Default)]
pub struct QtRuntime;

/// Thread-safe handle to the runtime.
#[derive(Debug, Clone, Copy)]
pub struct QtRuntimeHandle;

/// Cross-thread user-event proxy.
#[derive(Debug, Clone, Copy)]
pub struct QtEventLoopProxy;

/// Per-window dispatcher (stub: identity only).
#[derive(Debug, Clone, Copy)]
pub struct QtWindowDispatcher {
    id: WindowId,
}

/// Per-webview dispatcher (stub: label only).
#[derive(Debug, Clone)]
pub struct QtWebviewDispatcher {
    label: String,
}

/// Window attributes builder (records setters; nothing applied yet).
#[derive(Debug, Clone, Default)]
pub struct QtWindowBuilder {
    theme: Option<Theme>,
    has_icon: bool,
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
        }
    }

    fn center(self) -> Self {
        self
    }

    fn position(self, _x: f64, _y: f64) -> Self {
        self
    }

    fn inner_size(self, _width: f64, _height: f64) -> Self {
        self
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

    fn title<S: Into<String>>(self, _title: S) -> Self {
        self
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

    fn maximized(self, _maximized: bool) -> Self {
        self
    }

    fn visible(self, _visible: bool) -> Self {
        self
    }

    fn transparent(self, _transparent: bool) -> Self {
        self
    }

    fn decorations(self, _decorations: bool) -> Self {
        self
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
        _pending: PendingWindow<T, Self::Runtime>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self::Runtime>> {
        todo!("M4: create_window (QMainWindow)")
    }

    fn create_webview(
        &self,
        _window_id: WindowId,
        _pending: PendingWebview<T, Self::Runtime>,
    ) -> Result<DetachedWebview<T, Self::Runtime>> {
        todo!("M4: create_webview (QWebEngineView)")
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
        // M4: QApplication construction + URL scheme registration.
        Ok(Self)
    }

    fn new_any_thread(_args: RuntimeInitArgs) -> Result<Self> {
        // M4: Qt on a non-main thread (supported on Linux).
        todo!("M4: new_any_thread")
    }

    fn create_proxy(&self) -> Self::EventLoopProxy {
        QtEventLoopProxy
    }

    fn handle(&self) -> Self::Handle {
        QtRuntimeHandle
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &self,
        _pending: PendingWindow<T, Self>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self>> {
        todo!("M4: create_window (QMainWindow)")
    }

    fn create_webview(
        &self,
        _window_id: WindowId,
        _pending: PendingWebview<T, Self>,
    ) -> Result<DetachedWebview<T, Self>> {
        todo!("M4: create_webview (QWebEngineView)")
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
        callback(RunEvent::Exit);
        0
    }

    fn run<F: FnMut(RunEvent<T>) + 'static>(self, mut callback: F) {
        callback(RunEvent::Exit);
    }
}

impl<T: UserEvent> rt::WebviewDispatch<T> for QtWebviewDispatcher {
    type Runtime = QtRuntime;

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, _f: F) -> Result<()> {
        Ok(())
    }

    fn on_webview_event<F: Fn(&WebviewEvent) + Send + 'static>(&self, _f: F) -> WebviewEventId {
        0
    }

    fn with_webview<F: FnOnce(Box<dyn std::any::Any>) + Send + 'static>(&self, _f: F) -> Result<()> {
        Ok(())
    }

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn open_devtools(&self) {}

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn close_devtools(&self) {}

    #[cfg(any(debug_assertions, feature = "devtools"))]
    fn is_devtools_open(&self) -> Result<bool> {
        Ok(false)
    }

    fn url(&self) -> Result<String> {
        todo!("M4: url (QWebEnginePage::url)")
    }

    fn bounds(&self) -> Result<rt::dpi::Rect> {
        todo!("M4: bounds")
    }

    fn position(&self) -> Result<rt::dpi::PhysicalPosition<i32>> {
        todo!("M4: position")
    }

    fn size(&self) -> Result<rt::dpi::PhysicalSize<u32>> {
        todo!("M4: size")
    }

    fn navigate(&self, _url: url::Url) -> Result<()> {
        todo!("M4: navigate (QWebEnginePage::load)")
    }

    fn reload(&self) -> Result<()> {
        todo!("M4: reload")
    }

    fn print(&self) -> Result<()> {
        todo!("M4: print")
    }

    fn close(&self) -> Result<()> {
        todo!("M4: close")
    }

    fn set_bounds(&self, _bounds: rt::dpi::Rect) -> Result<()> {
        todo!("M4: set_bounds")
    }

    fn set_size(&self, _size: rt::dpi::Size) -> Result<()> {
        todo!("M4: set_size")
    }

    fn set_position(&self, _position: rt::dpi::Position) -> Result<()> {
        todo!("M4: set_position")
    }

    fn set_focus(&self) -> Result<()> {
        todo!("M4: set_focus")
    }

    fn hide(&self) -> Result<()> {
        todo!("M4: hide")
    }

    fn show(&self) -> Result<()> {
        todo!("M4: show")
    }

    fn eval_script<S: Into<String>>(&self, _script: S) -> Result<()> {
        todo!("M4: eval_script (QWebEnginePage::runJavaScript)")
    }

    fn eval_script_with_callback<S: Into<String>>(
        &self,
        _script: S,
        _callback: impl Fn(String) + Send + 'static,
    ) -> Result<()> {
        todo!("M4: eval_script_with_callback")
    }

    fn reparent(&self, _window_id: WindowId) -> Result<()> {
        todo!("M4: reparent")
    }

    fn cookies_for_url(&self, _url: url::Url) -> Result<Vec<Cookie<'static>>> {
        todo!("M4: cookies_for_url (QWebEngineCookieStore)")
    }

    fn cookies(&self) -> Result<Vec<Cookie<'static>>> {
        todo!("M4: cookies")
    }

    fn set_cookie(&self, _cookie: cookie::Cookie<'_>) -> Result<()> {
        todo!("M4: set_cookie")
    }

    fn delete_cookie(&self, _cookie: cookie::Cookie<'_>) -> Result<()> {
        todo!("M4: delete_cookie")
    }

    fn set_auto_resize(&self, _auto_resize: bool) -> Result<()> {
        todo!("M4: set_auto_resize")
    }

    fn set_zoom(&self, _scale_factor: f64) -> Result<()> {
        todo!("M4: set_zoom (QWebEnginePage::setZoomFactor)")
    }

    fn set_background_color(&self, _color: Option<Color>) -> Result<()> {
        todo!("M4: set_background_color")
    }

    fn clear_all_browsing_data(&self) -> Result<()> {
        todo!("M4: clear_all_browsing_data")
    }
}

impl<T: UserEvent> rt::WindowDispatch<T> for QtWindowDispatcher {
    type Runtime = QtRuntime;
    type WindowBuilder = QtWindowBuilder;

    fn run_on_main_thread<F: FnOnce() + Send + 'static>(&self, _f: F) -> Result<()> {
        Ok(())
    }

    fn on_window_event<F: Fn(&WindowEvent) + Send + 'static>(&self, _f: F) -> WindowEventId {
        0
    }

    fn scale_factor(&self) -> Result<f64> {
        todo!("M4: scale_factor")
    }

    fn inner_position(&self) -> Result<rt::dpi::PhysicalPosition<i32>> {
        todo!("M4: inner_position")
    }

    fn outer_position(&self) -> Result<rt::dpi::PhysicalPosition<i32>> {
        todo!("M4: outer_position")
    }

    fn inner_size(&self) -> Result<rt::dpi::PhysicalSize<u32>> {
        todo!("M4: inner_size")
    }

    fn outer_size(&self) -> Result<rt::dpi::PhysicalSize<u32>> {
        todo!("M4: outer_size")
    }

    fn is_fullscreen(&self) -> Result<bool> {
        todo!("M4: is_fullscreen")
    }

    fn is_minimized(&self) -> Result<bool> {
        todo!("M4: is_minimized")
    }

    fn is_maximized(&self) -> Result<bool> {
        todo!("M4: is_maximized")
    }

    fn is_focused(&self) -> Result<bool> {
        todo!("M4: is_focused")
    }

    fn is_decorated(&self) -> Result<bool> {
        todo!("M4: is_decorated")
    }

    fn is_resizable(&self) -> Result<bool> {
        todo!("M4: is_resizable")
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
        todo!("M4: is_visible")
    }

    fn is_enabled(&self) -> Result<bool> {
        Ok(true)
    }

    fn is_always_on_top(&self) -> Result<bool> {
        Ok(false)
    }

    fn title(&self) -> Result<String> {
        todo!("M4: title")
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

    fn gtk_window(&self) -> Result<gtk::ApplicationWindow> {
        // Qt runtime has no GTK window; callers must use `window_handle`.
        Err(Error::CreateWindow)
    }

    fn default_vbox(&self) -> Result<gtk::Box> {
        Err(Error::CreateWindow)
    }

    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        Err(HandleError::Unavailable)
    }

    fn theme(&self) -> Result<Theme> {
        Ok(Theme::Light)
    }

    fn center(&self) -> Result<()> {
        todo!("M4: center")
    }

    fn request_user_attention(&self, _request_type: Option<UserAttentionType>) -> Result<()> {
        Ok(())
    }

    fn create_window<F: Fn(RawWindow) + Send + 'static>(
        &mut self,
        _pending: PendingWindow<T, Self::Runtime>,
        _after_window_creation: Option<F>,
    ) -> Result<DetachedWindow<T, Self::Runtime>> {
        todo!("M4: create_window (QMainWindow)")
    }

    fn create_webview(
        &mut self,
        _pending: PendingWebview<T, Self::Runtime>,
    ) -> Result<DetachedWebview<T, Self::Runtime>> {
        todo!("M4: create_webview (QWebEngineView)")
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

    fn set_title<S: Into<String>>(&self, _title: S) -> Result<()> {
        todo!("M4: set_title")
    }

    fn maximize(&self) -> Result<()> {
        todo!("M4: maximize")
    }

    fn unmaximize(&self) -> Result<()> {
        todo!("M4: unmaximize")
    }

    fn minimize(&self) -> Result<()> {
        todo!("M4: minimize")
    }

    fn unminimize(&self) -> Result<()> {
        todo!("M4: unminimize")
    }

    fn show(&self) -> Result<()> {
        todo!("M4: show")
    }

    fn hide(&self) -> Result<()> {
        todo!("M4: hide")
    }

    fn close(&self) -> Result<()> {
        todo!("M4: close")
    }

    fn destroy(&self) -> Result<()> {
        todo!("M4: destroy")
    }

    fn set_decorations(&self, _decorations: bool) -> Result<()> {
        todo!("M4: set_decorations")
    }

    fn set_shadow(&self, _enable: bool) -> Result<()> {
        Ok(())
    }

    fn set_always_on_bottom(&self, _always_on_bottom: bool) -> Result<()> {
        Ok(())
    }

    fn set_always_on_top(&self, _always_on_top: bool) -> Result<()> {
        todo!("M4: set_always_on_top")
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

    fn set_size(&self, _size: rt::dpi::Size) -> Result<()> {
        todo!("M4: set_size")
    }

    fn set_min_size(&self, _size: Option<rt::dpi::Size>) -> Result<()> {
        Ok(())
    }

    fn set_max_size(&self, _size: Option<rt::dpi::Size>) -> Result<()> {
        Ok(())
    }

    fn set_size_constraints(
        &self,
        _constraints: WindowSizeConstraints,
    ) -> Result<()> {
        todo!("M4: set_size_constraints")
    }

    fn set_position(&self, _position: rt::dpi::Position) -> Result<()> {
        todo!("M4: set_position")
    }

    fn set_fullscreen(&self, _fullscreen: bool) -> Result<()> {
        todo!("M4: set_fullscreen")
    }

    fn set_fullscreen_on_monitor(
        &self,
        _position: rt::dpi::PhysicalPosition<f64>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_focus(&self) -> Result<()> {
        todo!("M4: set_focus")
    }

    fn set_focusable(&self, _focusable: bool) -> Result<()> {
        Ok(())
    }

    fn set_icon(&self, _icon: Icon) -> Result<()> {
        todo!("M4: set_icon (QIcon)")
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
        todo!("M4: start_dragging (QWindow::startSystemMove)")
    }

    fn start_resize_dragging(&self, _direction: ResizeDirection) -> Result<()> {
        todo!("M4: start_resize_dragging (QWindow::startSystemResize)")
    }

    fn set_badge_count(
        &self,
        _count: Option<i64>,
        _desktop_filename: Option<String>,
    ) -> Result<()> {
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

/// Check the trait surface compiles with `tauri::Builder` integration later.
#[cfg(test)]
fn _icons_are_cow() {
    let _: Cow<'_, [u8]> = Icon {
        rgba: Cow::Borrowed(&[]),
        width: 0,
        height: 0,
    }
    .rgba;
}
