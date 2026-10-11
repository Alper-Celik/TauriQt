<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT -->

# TauriQt — Plan

Goal: a Qt-based runtime for Tauri 2 on Linux — Qt windowing (replace tao) + QtWebEngine
(replace webkit2gtk), implemented as a custom `tauri::runtime::Runtime`.

Status (2026-10-11, end of the day the repository was written):

- **M0 done** — sample app (Rust → cxx-qt → `QApplication` + `QWebEngineView`, google.com)
  renders on Wayland.
- **M1 done** — `tauri-runtime-qt` implements the full trait surface and backs it with real
  Qt code: `QApplication` bootstrap + URL-scheme registration (`qt_app.cpp`), window
  registry/geometry/events (`qt_window.cpp`), `QWebEngineView` lifecycle + devtools
  (`qt_webview.cpp`), and the `ipc`/`tauri`/`asset` scheme handler with the reply-job
  lifetime recipe (`qt_scheme.cpp`). Glue is plain `cxx` + `qt-build-utils` (cxx-qt's
  initializer machinery cannot link from a library crate).
- **M2/M3 done** — `tauri://` and `asset://` serving, window/event plumbing, `QTimer`-driven
  proxy dispatch, monitors via `QScreen`; cookies have a mirror plus `cookies_for_url`.
- **M4 done except for the tail** — the trait surface is implemented and exercised by the
  demo's twelve commands. Pure logic now lives in `tauri-runtime-qt-core` (no Qt, no `cxx`),
  so it links and tests normally: `cargo test -p tauri-runtime-qt-core` runs **65 unit
  tests**. The remaining `WindowDispatch` methods return `Err(NotSupported)` where Qt has no
  equivalent — never silent no-ops.
- **M5 done** — `nix build .#tauriqt` packages the app with Qt, and the three flake checks
  (`smoke`, `wayland-smoke`, `sway-oracle`) plus `scripts/coverage.sh` grade it; CI runs
  them on x86_64 and aarch64.
- **Beyond the plan**: the repository also carries a coverage pipeline (merged llvm lcov for
  Rust + C++, genhtml and ReportGenerator HTML) and an integrations tier that runs *other
  people's* apps against this runtime — [Readest](integrations/readest/README.md) boots on
  it, its `invoke()` reaches its Rust and its own 183-test suite passes. Running it is what
  surfaced the `tauri-runtime` 2.12 / 2.11 skew below.
- App side: `tauriqt` is a Tauri 2 app built **without the `wry` feature** on Linux, with
  wry wired in as a per-target fallback for Windows/macOS (and `--features wry` on Linux).

## Milestones

### M0 — Feasibility spike ✅
- cxx-qt 0.10 toolchain, nix flake (merged-Qt prefix + qmake shim, `QMAKE` pinned)
- QtWebEngine renders on Wayland

### M1 — IPC round-trip ✅
- `QWebEngineUrlScheme::registerScheme` for `ipc`, `tauri`, `asset` before the
  `QApplication` (flags: `SecureScheme | CorsEnabled | FetchApiAllowed`, `Syntax::Host`)
- `QWebEngineUrlSchemeHandler` → cxx bridge → the `uri_scheme_protocols` tauri registers
  per webview (handlers take the **webview label**, not the scheme name)
- Async completion: tauri resolves protocols asynchronously, so a request may return
  `handled == 2` (pending); C++ keeps the job alive under a `request_id` and Rust completes
  it through `qt_scheme_respond`
- **QtWebEngine never delivers custom-protocol POST bodies** (`requestBody()` is an
  already-ended, empty sequential device — verified by probing at 0/50/200/500 ms). The
  runtime therefore ships its own invoke system (`INVOKE_SYSTEM_SCRIPT`): the payload
  travels base64-encoded in the `X-Tauriqt-Payload` header, and the runtime turns that
  header back into the request body before tauri's IPC handler sees it. Payload size is
  bounded by the engine's request-header limit (~256 KB) — chunking is future work.
- Exit criterion met: `invoke("greet", { name: "Qt" })` → `ipc POST ipc://localhost/greet
  -> 200` → `"Hello, Qt! — from linux"` rendered by the page (title `INVOKE_OK`)

### M2 — Frontend serving ✅
- `tauri://localhost` scheme serving the built frontend (`dist/`)
- `asset://localhost` with default port 0 semantics (like wry)
- init scripts via `QWebEngineProfile::scripts` (`QWebEngineScript` at `DocumentCreation`)

### M3 — Window + event plumbing (Qt replaces tao) ✅
- `QMainWindow`/`QWidget` hosting the `QWebEngineView`
- Events: close-request, resize, focus, drag-drop → `WindowEvent`/`WebviewEvent`
- `QTimer`-driven proxy dispatch for `EventLoopProxy`
- Monitor info via `QScreen`

### M4 — tauri-runtime-qt crate (the API surface below) 🔶 (tail: Qt-less methods)
- Implement `tauri_runtime` traits, wire `tauri::Builder<QtRuntime>`
- Unknown/unneeded `WindowDispatch` methods: return `Err(Error::NotSupported)` — stubbed,
  never silent no-ops

### M5 — Nix packaging + polish ✅
- `QtWebEngineProcess` discovery, sandbox wrapper if needed
- Bundle size sanity check

## MVP API surface

Full per-method inventory with Qt mappings and ✅/🔶/⛔ classification lives in
[RUNTIME-TRAITS.md](RUNTIME-TRAITS.md). Method counts: `Runtime` 20, `RuntimeHandle` 21,
`EventLoopProxy` 1, `WebviewDispatch` 31, `WindowDispatch` 79, `WindowBuilder` ~42.


## Risks / open questions
- tauri-runtime trait churn across tauri minor releases — **no longer hypothetical**: 2.12
  added the required `WindowDispatch::set_fullscreen_on_monitor`, which broke a consumer
  pinned to 2.11.3 (Readest) until it moved behind the default-on `tauri-runtime-2-12`
  feature. Expect more of this, and keep such additions feature-gated.
- LGPLv3 (QtWebEngine) → dynamic linking only; fine for local use, matters on distribution
- Chromium sandbox on NixOS: **resolved** — no wrapper needed, but QtWebEngine aborts
  instead of falling back when there is no GL device, so headless runs set
  `QTWEBENGINE_CHROMIUM_FLAGS=--no-sandbox --disable-gpu --disable-gpu-compositing`
  (docs/nix-qt-gotchas.md). Related, and now documented as a check: an environment with no
  `/dev/dri` at all never gets a *mapped* window, because QtWebEngine's `QQuickWidget`-backed
  flush cannot commit a `wl_buffer` (docs/wayland-smoke-findings.md, verdict 5).

## Current coverage — runtime demo (`tauriqt` app)

`tauriqt/src/main.rs` registers twelve demo commands that exercise the runtime end-to-end
and print their results to the terminal via `report`:

| Command | Exercises |
|---|---|
| `win_info` | `inner_size`, `outer_size`, `inner_position`, `outer_position`, `scale_factor`, `title`, `is_fullscreen`, `is_minimized`, `is_maximized`, `is_focused`, `is_decorated`, `is_resizable`, `is_visible`, `is_always_on_top`, `is_maximizable`, `is_minimizable`, `is_closable` |
| `monitors` | `available_monitors`, `primary_monitor`, `current_monitor` |
| `set_fullscreen` | `WindowDispatch::set_fullscreen` |
| `set_title` | `WindowDispatch::set_title` |
| `report` | `println!` breadcrumb channel for the page |
| `cookie_sandbox_url` / `cookies_dump` | cookie mirror, `cookies_for_url` |
| `cookie_set` / `cookie_delete` / `cookies_clear` | `set_cookie`, `delete_cookie`, `clear_all_browsing_data` |
| `platform` | `platform_name()` — which Qt platform plugin is running, reported with the verdict |

Each probed method is reported independently: unsupported/unimplemented methods
(delegates that still return `Err`, e.g. `unimplemented("WindowDispatch::…")` in
`src/ops/`) show up as `field=ERR(message)` inside the command's returned string,
and rejected commands (`set_fullscreen`/`set_title` surface the runtime error as
an invoke rejection) are caught by the page. The page sets
`document.title = DEMO_OK` when nothing failed, otherwise
`DEMO_PARTIAL: <failures>`; because titles are forwarded to the runtime as
events, the outcome is visible in the terminal as
`[tauri-runtime-qt] webview <n> event 4: DEMO_OK`.

Snapshot of the still-`Err` delegates at the time this demo landed (parallel
workers complete these; see `RUNTIME-TRAITS.md`): `ops/window.rs`
geometry/state reads+`set_fullscreen`/`set_position`/…, `ops/screen.rs`
monitor+cursor, `ops/webview.rs` bounds/position/size/focus/reparent/
`clear_all_browsing_data`, `ops/runtime.rs` `send_event`/`request_exit`.
