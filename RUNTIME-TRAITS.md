<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT -->

# tauri-runtime-qt — Trait Implementation Inventory

Extracted from `crates/tauri-runtime/src/{lib.rs,window.rs}` (tauri dev, 2026-10-11).
Legend: ✅ functional in MVP · 🔶 trivial / near-free · ⛔ stub → `Err(Error::NotSupported)`

Threading model for all traits: dispatchers must be `Send + Sync + Clone`; every method
marshals to the main thread via `QMetaObject::invokeMethod(..., QueuedConnection)` onto the
runtime's dispatcher object. State lives in Qt objects; no manual locking.

Where a mapping is *pure* — cookie matching, scheme request decode / response encode,
geometry and DPR conversions, resize codes, `WindowConfig` → builder parameters — the code
now lives in `tauri-runtime-qt-core`, with the Qt-side `ops/` (or `qt_*`) function a thin
delegate. That crate has no Qt, no `cxx` and no build script, so its logic is testable with
plain `cargo test -p tauri-runtime-qt-core` (65 unit tests today); the tables below name the
Qt-side call.

---

## 1. `Runtime<T>` — 20 methods (required: all)

| Method | Qt mapping | MVP |
|---|---|---|
| `new(args)` / `new_any_thread(args)` | construct `QApplication` (argc/argv from `RuntimeInitArgs`), register URL schemes (must precede any GUI object!) | ✅ |
| `create_window(pending, after_window_creation)` | build `QMainWindow`, store in registry, return `DetachedWindow` | ✅ |
| `create_webview(window_id, pending, after_window_creation)` | build `QWebEngineView` + `QWebEngineProfile` w/ scheme handlers | ✅ |
| `create_proxy()` | clone dispatcher handle | ✅ |
| `handle()` | `RuntimeHandle` | ✅ |
| `run(callback)` | `QApplication::exec()`, map `RunEvent` per callback | ✅ |
| `run_return(callback)` | exec + return exit code | ✅ |
| `run_iteration(callback)` | `processEvents()` + pump queued events | ✅ |
| `primary_monitor` / `monitor_from_point` / `available_monitors` | `QScreen` list (returns `Option`, not `Result`) | ✅ |
| `cursor_position()` | `QCursor::pos()` | 🔶 |
| `set_theme(Option<Theme>)` | `QApplication::setPalette` / style hints | 🔶 |
| `set_activation_policy` / `set_activate_ignoring_other_apps` / `set_dock_visibility` | macOS-only → no-op Ok | ⛔ (harmless) |
| `show` / `hide` (app-level) | `QApplication::setQuitOnLastWindowClosed` semantics; MVP: no-op | ⛔ |
| `set_device_event_filter` | no-op (Qt doesn't expose raw device events) | ⛔ |

## 2. `RuntimeHandle<T>` — 21 methods (required: all)

Same operations as `Runtime` but callable from any thread (uses the proxy/queued
invocation). MVP mapping:

- `create_proxy`, `create_window`, `create_webview`, `run_on_main_thread`, `send_event`
  (proxy send), monitors, `cursor_position`, `set_theme`, `show`, `hide` → ✅ (mirrors Runtime)
- `set_activation_policy`, `set_dock_visibility`, `request_exit` → `request_exit` = ✅
  (`QCoreApplication::exit(code)`); others ⛔
- `display_handle` → ✅ raw window display handle (raw-window-handle)
- `find_class` / `run_on_android_context` / `fetch_data_store_identifiers` /
  `remove_data_store` (Android/iOS) → ⛔

## 3. `EventLoopProxy<T>` — 1 method

- `send_event(T)` → push into thread-safe queue + wake main loop
  (`QMetaObject::invokeMethod` with a queued slot that drains the queue → emits
  `RunEvent::UserEvent`). ✅

## 4. `WebviewDispatch<T>` — 31 methods (required: all)

| Method | Qt mapping | MVP |
|---|---|---|
| `run_on_main_thread` | queued invocation | ✅ |
| `on_webview_event` | register `QWebEnginePage` load hooks (`loadStarted/Finished`, `urlChanged`, `windowCloseRequested`, `renderProcessTerminated`) → `WebviewEvent` | ✅ |
| `with_webview` | pass `QWebEnginePage*` boxed as `Any` | ✅ (Qt-typed) |
| `open_devtools` / `close_devtools` / `is_devtools_open` | Qt6: separate `QWebEngineView` devtools page; track state | 🔶 (may stub `is_devtools_open` state in Rust) |
| `url()` | `QWebEnginePage::url()` | ✅ |
| `bounds` / `position` / `size` / `set_bounds` / `set_size` / `set_position` | view widget geometry, logical→physical scale | ✅ |
| `navigate(Url)` / `reload()` / `close()` | `load(QUrl)`, `reload()`, delete view + unregister | ✅ |
| `print()` | `QWebEngineView::page()->printToPdf`? No — `page()->action(Print)`… MVP ⛔ if Qt6 lacks direct API (verify) | ⛔ |
| `set_focus()` / `show()` / `hide()` | widget API | ✅ |
| `eval_script` | `page()->runJavaScript(js)` | ✅ |
| `eval_script_with_callback` | `runJavaScript(js, QWebEngineCallback)` → bridge callback to Rust thread | ✅ (async note below) |
| `reparent(window_id)` | `QWebEngineView::setParent` / re-layout into target window | 🔶 (single-window MVP: stub) |
| `cookies_for_url` / `cookies` / `set_cookie` / `delete_cookie` | `QWebEngineProfile::cookieStore()` (async API → queue + callback) | 🔶 |
| `set_auto_resize` | widget auto-resize flag | ✅ |
| `set_zoom` | `page()->setZoomFactor` | ✅ |
| `set_background_color` | `page()->setBackgroundColor` | ✅ |
| `clear_all_browsing_data` | profile data paths + `QWebEngineProfile` clearing (Qt 6.7+: `clearHttpCache` etc.) | 🔶 |

### Note on `WebviewEvent`
`tauri_runtime::window::WebviewEvent` (2.12) has a single variant, `DragDrop`; page-load and
title events must be routed through the `on_page_load_handler` / `document_title_changed_handler`
tauri installs on `PendingWebview`, not through `on_webview_event`.

### IPC (not a trait method — lives in the runtime's scheme handlers)
- `ipc://localhost/{cmd}` POST → `QWebEngineUrlSchemeHandler` → Tauri core invoke path
- response written through `QWebEngineUrlRequestJob` (reply + content-type headers)
- `Tauri-Invoke-Key` validation identical to wry's scheme handler logic

## 5. `WindowDispatch<T>` — 79 methods (required: all)

| Group | Methods | MVP |
|---|---|---|
| Geometry read | `scale_factor`, `inner_position`, `outer_position`, `inner_size`, `outer_size` | ✅ |
| Geometry write | `set_size`, `set_position`, `center`, `set_min_size`, `set_max_size`, `set_size_constraints` | ✅ (constraints via `QWindow` min/max) |
| State read | `is_fullscreen`, `is_minimized`, `is_maximized`, `is_focused`, `is_decorated`, `is_resizable`, `is_maximizable`, `is_minimizable`, `is_closable`, `is_visible`, `is_enabled`, `is_always_on_top`, `title`, `theme` | ✅ (flags mirrored in Rust state; Qt doesn't expose `is_closable`/`is_maximizable` on Linux — tracked flags) |
| State write | `maximize`, `unmaximize`, `minimize`, `unminimize`, `show`, `hide`, `close`, `destroy`, `set_resizable`, `set_enabled`, `set_maximizable`, `set_minimizable`, `set_closable`, `set_title`, `set_decorations`, `set_shadow`, `set_fullscreen`, `set_focus`, `set_icon` | ✅ (`set_shadow`/`set_icon` 🔶 in spirit, both implemented; `set_icon` refuses only on malformed icon data) |
| Stacking (conditional) | `set_always_on_top`, `set_always_on_bottom` | ✅ on X11; **`Err` on Wayland** — a compositor owns stacking and Qt's `Qt::WindowStaysOnTopHint` is ignored there (QTBUG-73456). The error says so rather than pretending |
| Events | `on_window_event` (close-request, focus, resize/move, fullscreen), `request_redraw`, `draw` | ✅ |
| Threading | `run_on_main_thread` | ✅ |
| Monitors | `current_monitor`, `primary_monitor`, `monitor_from_point`, `available_monitors` | ✅ (`QScreen::geometry`) |
| Nested creation | `create_window`, `create_webview` | ✅ |
| Platform handles | `window_handle` (`WId` via `winId()` + raw-window-handle), `gtk_window`/`default_vbox` (GTK-only), `activity_name`/`scene_identifier` (Android/macOS) | `window_handle` ✅, rest ⛔ |
| Misc | `set_background_color`, `set_title_bar_style`, `set_traffic_light_position`, `set_theme` | ✅ (macOS-only trait methods such as `set_simple_fullscreen` are not part of the trait on Linux, so they are absent from the impl rather than stubbed) |
| Misc (⛔) | `request_user_attention` (X11 urgency hint), `set_content_protected`, `set_visible_on_all_workspaces`, `set_fullscreen_on_monitor`, `set_focusable`, `set_skip_taskbar`, `gtk_window`, `default_vbox` | `Err(unimplemented)`. `set_fullscreen_on_monitor` is also *version-gated*: tauri-runtime 2.12 added it as a required method, so it only exists when the `tauri-runtime-2-12` feature is on (default) |
| Cursor | `set_cursor_grab`, `set_cursor_visible`, `set_cursor_icon`, `set_ignore_cursor_events` | ⛔ all four are `Err(unimplemented)` today (`set_cursor_position` is not a method in this `tauri-runtime`) |
| Drag | `start_dragging` (→ `QWindow::startSystemMove`), `start_resize_dragging` (→ `startSystemResize`) | ✅ |
| Taskbar extras | `set_badge_count`, `set_badge_label`, `set_overlay_icon`, `set_progress_bar` | ⛔ |

### Reads must not fail — the one rule the table cannot show

Every geometry/state/monitor **read** in the table above answers, and that is a rule rather
than a coincidence: callers treat reads as always available, so a single `Err` aborts a whole
plugin's start-up work. `tauri-plugin-window-state`'s restore queries `outer_position()`,
`is_maximized()`, `is_visible()`, `is_decorated()` and `is_fullscreen()?`, and it aborts on
the first error — with the position getters refusing off-xcb, that abort meant the window was
never mapped for Readest at all (`integrations/readest/README.md`, "The hidden main window").
Positions now answer everywhere (the compositor's own placement simply is not readable, so
Qt's record of the request is what comes back; see `docs/wayland-ci.md`).

Two reads still refuse, deliberately, and only when the platform genuinely has no value to
give — never by fabricating one: `outer_size` when Qt reports an empty frame rectangle, and
`theme` when Qt reports the colour scheme as unknown. The demo probes the former and reports
`DEMO_OK`, i.e. no `field=ERR(…)`, on xcb, offscreen and Wayland alike, so neither fires in
practice on these platforms; if one ever does for an app, it is the same failure class as the
position bug and should be treated as such.

The `Err` surface is therefore exactly: the `Err(unimplemented)` methods named above (twelve
that never return `Ok`), the Wayland stacking hints, dragging when Qt declines
(`startSystemMove` false, or an invalid resize direction), `set_icon` on malformed icon data,
those two reads, and `require_window` — an unknown window id, which every method shares.

These lists are read off the code rather than from memory: the table above drifted from it
once already (a row claiming `set_skip_taskbar` and `set_focusable` were implemented, a cursor
row claiming five methods when this `tauri-runtime` has four). When the impl changes, re-derive
them — `grep` for methods whose body returns `Err(` and never `Ok(` is the whole test.

## 6. `WindowBuilder` (trait `WindowBuilder: WindowBuilderBase`) — ~42 builder methods

All are plain setters consumed at `create_window`. Trivial: store into a
`WindowConfig`-like struct, applied in `Runtime::create_window`.
Functional for MVP: `new`, `with_config`, `center`, `position`, `inner_size`,
`min_inner_size`, `max_inner_size`, `inner_size_constraints`, `resizable`,
`maximizable`, `minimizable`, `closable`, `title`, `fullscreen`, `focused`,
`focusable`, `maximized`, `visible`, `decorations`, `always_on_top`,
`always_on_bottom`, `visible_on_all_workspaces`, `content_protected`, `shadow`,
`skip_taskbar`, `background_color`, `theme`, `drag_and_drop`.
Stubs (no-op storing, no Qt effect): `icon` (needs image decode — 🔶),
`transparent` (🔶 `Qt::WA_TranslucentBackground`), `prevent_overflow*`,
`title_bar_style`, `traffic_light_position`, `hidden_title`, `tabbing_identifier`,
`owner`/`parent`/`transient_for` (🔶 multi-window MVP), platform-specific duplicates.

Note: there is **no `WebviewBuilder` trait** — webview config rides
`WebviewAttributes` + `PendingWebview`; the runtime just consumes it.

---

## Cross-cutting notes
- `T::UserEvent` = tauri's event enum; proxy drains queue into `RunEvent::UserEvent(T)`
- All `on_*_event` registrations happen on the main thread; tauri core stores the
  returned event ids and removes handlers itself
- `Error` enum: `#[non_exhaustive]`, reuse existing variants; add `NotSupported` if absent
- The webview widget must fill the window: `setCentralWidget`-equivalent; resize handled
  by Qt layout (no manual `SetWindowPos` dance like CEF on Windows)

---

## Demo coverage — wired vs. still `Err` (2026-10-11)

The `tauriqt` app registers demo commands (`win_info`, `monitors`,
`set_fullscreen`, `set_title`, `report`; see `PLAN.md` → "Current coverage").
Their per-method outcome is the runtime's truth at this revision:

**Wired (return real values / real effects).** `WindowDispatch::scale_factor`,
`show`, `hide`, `close`, `destroy`, `set_title`, `set_size`, `center`,
`set_decorations`, `set_maximized`, `is_visible`, plus the whole scheme/IPC +
`WebviewDispatch::url`/`navigate`/`reload`/`eval_script` path.

**Still `Err` (delegates to `ops::unimplemented(...)`).** In this worktree these
are reported by the demo as `field=ERR(message)` and are being filled by the
window/screen/webview/runtime workers in parallel:

- `ops/window.rs`: `inner_size`, `outer_size`, `inner_position`,
  `outer_position`, `title`, `theme`, `is_fullscreen`, `is_minimized`,
  `is_maximized`, `is_focused`, `is_decorated`, `is_resizable`, `is_enabled`,
  `is_always_on_top`, `set_fullscreen`, `minimize`, `unminimize`, `set_min_size`,
  `set_max_size`, `set_size_constraints`, `set_icon`, `set_focus`,
  `set_resizable`, `set_enabled`, `start_dragging`, `start_resize_dragging`,
  `set_always_on_bottom`, `set_always_on_top`, `set_background_color`,
  `set_position`.
- `ops/screen.rs`: `available_monitors`, `primary_monitor`,
  `monitor_from_point`, `monitor_for_window` (`current_monitor`),
  `cursor_position`.
- `ops/webview.rs`: `bounds`, `position`, `size`, `set_size`, `set_position`,
  `set_focus`, `eval_script_with_callback`, `reparent`,
  `clear_all_browsing_data`.
- `ops/runtime.rs`: `EventLoopProxy::send_event`, `RuntimeHandle::request_exit`.

Because the demo probes each getter independently and `set_fullscreen` /
`set_title` surface failures as invoke rejections, the demo result line is a
direct, non-fabricated coverage report at run time — it must only be updated as
the corresponding `ops` delegates are implemented.
