# TauriQt — Plan

Goal: a Qt-based runtime for Tauri 2 on Linux — Qt windowing (replace tao) + QtWebEngine
(replace webkit2gtk), implemented as a custom `tauri::runtime::Runtime`.

Status: **M0 done** — sample app (Rust → cxx-qt → `QApplication` + `QWebEngineView`,
google.com) runs buttery smooth on Wayland (validated 2026-10-11).

## Milestones

### M0 — Feasibility spike ✅
- cxx-qt 0.10 toolchain, nix flake (merged-Qt prefix + qmake shim, `QMAKE` pinned)
- QtWebEngine renders on Wayland

### M1 — IPC round-trip (core feasibility of the runtime)
- `QWebEngineUrlScheme::registerScheme` for `ipc`, `tauri`, `asset` at startup
  (flags: `SecureScheme | CorsEnabled | FetchApiAllowed`)
- `QWebEngineUrlSchemeHandler` for `ipc://localhost/{cmd}` → Tauri command dispatch → response
- Frontend: raw `fetch("ipc://localhost/greet")` (no tauri JS glue yet)
- Exit criterion: Rust `greet(name)` returns "Hello, {name}!" in the page

### M2 — Frontend serving
- `tauri://localhost` scheme serving the built frontend (`dist/`)
- `asset://localhost` with default port 0 semantics (like wry)
- init scripts via `QWebEngineProfile::scripts` (`QWebEngineScript` at `DocumentCreation`)

### M3 — Window + event plumbing (Qt replaces tao)
- `QMainWindow`/`QWidget` hosting the `QWebEngineView`
- Events: close-request, resize, focus, drag-drop → `WindowEvent`/`WebviewEvent`
- `QTimer`-driven proxy dispatch for `EventLoopProxy`
- Monitor info via `QScreen`

### M4 — tauri-runtime-qt crate (the API surface below)
- Implement `tauri_runtime` traits, wire `tauri::Builder<QtRuntime>`
- Unknown/unneeded `WindowDispatch` methods: return `Err(Error::NotSupported)` — stubbed,
  never silent no-ops

### M5 — Nix packaging + polish
- `QtWebEngineProcess` discovery, sandbox wrapper if needed
- Bundle size sanity check

## MVP API surface

Reference: `tauri-runtime` (docs.rs), tauri-runtime-wry as the only complete impl.
Full `WindowDispatch` is ~78 methods; MVP = functional subset below, everything else stubbed.

### Runtime (all required, functional for MVP)
| Method | Notes |
|---|---|
| `new` / `new_any_thread` | create `QCoreApplication`-family + dispatcher |
| `run` / `run_return` | `QCoreApplication::exec()` |
| `run_iteration` | `processEvents()` |
| `create_window` | build `QMainWindow` + attach webview |
| `create_webview` | build `QWebEngineView`, wire schemes/profile |
| `create_proxy` | cross-thread event queue |
| `handle` | `RuntimeHandle` |
| `set_theme`, `cursor_position`, monitor queries | via `QScreen` / style hints |

### RuntimeHandle
| Method | Notes |
|---|---|
| `create_proxy`, `create_window`, `create_webview` | same as Runtime, thread-safe |

### EventLoopProxy
| Method | Notes |
|---|---|
| `send_event` | `QMetaObject::invokeMethod` queued → wakes `run` |

### WindowDispatch — functional subset
| Group | Methods |
|---|---|
| Geometry | `inner_position`, `outer_position`, `inner_size`, `outer_size`, `scale_factor`, `set_bounds`, `set_size`, `set_position`, `center`, `set_resizable`, `set_minimizable`, `set_maximizable`, `set_closable`, `is_*` getters |
| State | `minimize`, `maximize`, `unmaximize`, `unminimize`, `is_minimized`, `is_maximized`, `is_fullscreen`, `set_fullscreen`, `hide`, `show`, `is_visible`, `set_focus`, `is_focused` |
| Decoration | `set_decorations`, `is_decorated`, `set_always_on_top`, `set_always_on_bottom`, `set_title`, `title`, `set_maximized` |
| Events/dispatch | `request_redraw`, `draw`, `on_window_event` |
| Threading | `run_on_main_thread` (queued connection), `current_monitor`, `primary_monitor`, `available_monitors`, `monitor_from_point` |
| Platform handles | `window_handle` (`HWND`-equivalent: `WId` from `winId()`), `gtk_window` → n/a stub |

### WindowDispatch — stubbed for MVP
Cursor icons/grab, content protection, progress/badge/overlay, cursor moves, dragging
(`start_dragging` may be trivial via `QWindow::startSystemMove`), size constraints
(`set_size_constraints` via `QWindow::setMaximumSize` etc. — stretch goal), theme setting.

### WebviewDispatch — functional subset
| Group | Methods |
|---|---|
| Navigation/content | `navigate`, `url`, `reload`, `eval_script`, `eval_script_with_callback`, `print` |
| Geometry | `bounds`, `set_bounds`, `set_size`, `set_position`, `inner_size`, `outer_size` |
| Visibility | `show`, `hide`, `set_focus`, `set_background_color` |
| IPC | internal: scheme handler → command dispatch (not part of the trait) |
| Web engine | `zoom`, `set_zoom`, `cookies*`, `set_cookie`, `delete_cookie`, `clear_all_browsing_data` |
| Devtools | `open_devtools`, `close_devtools`, `is_devtools_open` |
| Threading | `run_on_main_thread`, `with_webview` (expose `QWebEnginePage`/`QWebEngineView` raw pointer) |
| Lifecycle | `reparent`, `close` |

### Known limitations accepted for MVP
- `eval_script_with_callback`: `runJavaScript` is callback/async — bridged via queued signal
- `with_webview` returns Qt types; Tauri plugins expecting WebKitGTK handles won't work
- No mobile, no `build_as_child` (QtWidgets makes multi-webview-in-window easy later)
- Tray/menu/window attributes that tao covers: implemented incrementally in M3/M4 as Tauri
  core requests them

## Risks / open questions
- tauri-runtime trait churn across tauri minor releases (rebase, like wrymium)
- LGPLv3 (QtWebEngine) → dynamic linking only; fine for local use, matters on distribution
- Chromium sandbox on NixOS: verify outside agent sandbox (worked on real session for the
  sample; revisit if `QtWebEngineProcess` needs a wrapper)
