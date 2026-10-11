# TauriQt — Plan

Goal: a Qt-based runtime for Tauri 2 on Linux — Qt windowing (replace tao) + QtWebEngine
(replace webkit2gtk), implemented as a custom `tauri::runtime::Runtime`.

Status (2026-10-11):

- **M0 done** — sample app (Rust → cxx-qt → `QApplication` + `QWebEngineView`, google.com)
  runs buttery smooth on Wayland.
- **M4 in progress** — `tauri-runtime-qt` implements the full trait surface and now backs it
  with real Qt code: `QApplication` bootstrap + URL-scheme registration (`qt_app.cpp`),
  window registry/geometry/events (`qt_window.cpp`), `QWebEngineView` lifecycle + devtools
  (`qt_webview.cpp`), and the `ipc`/`tauri`/`asset` scheme handler with the reply-job lifetime
  recipe (`qt_scheme.cpp`). Glue is plain `cxx` + `qt-build-utils` (cxx-qt's initializer
  machinery cannot link from a library crate).
- **M1 wiring** — Rust routes scheme requests into the `uri_scheme_protocols` tauri registers
  per webview, and maps responses back (`Tauri-Response`, CORS expose-headers, content type).
- App side: `tauriqt` is now a Tauri 2 app built **without the `wry` feature** on Linux, with
  wry wired in as a per-target fallback for Windows/macOS (and `--features wry` on Linux).

Known limits at this stage: cookies/`clear_all_browsing_data`/`eval_script_with_callback`/
`reparent` return errors; scheme responses that tauri resolves asynchronously are reported
unhandled (the bridge call is synchronous); binary IPC bodies are carried as `str`.

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

Full per-method inventory with Qt mappings and ✅/🔶/⛔ classification lives in
[RUNTIME-TRAITS.md](RUNTIME-TRAITS.md). Method counts: `Runtime` 20, `RuntimeHandle` 21,
`EventLoopProxy` 1, `WebviewDispatch` 31, `WindowDispatch` 79, `WindowBuilder` ~42.


## Risks / open questions
- tauri-runtime trait churn across tauri minor releases (rebase, like wrymium)
- LGPLv3 (QtWebEngine) → dynamic linking only; fine for local use, matters on distribution
- Chromium sandbox on NixOS: verify outside agent sandbox (worked on real session for the
  sample; revisit if `QtWebEngineProcess` needs a wrapper)
