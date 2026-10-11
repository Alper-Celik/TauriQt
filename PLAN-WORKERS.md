# Parallel work plan — closing the coverage gaps

Goal: implement the missing runtime surface with isolated workers (one git worktree each),
disjoint file ownership, and an integrator who owns the shared files.

## Why this shape

- Rust allows **one** `impl Trait for Type` per type, so `Runtime`/`WindowDispatch`/
  `WebviewDispatch` implementations cannot be split across workers.
- Therefore: the integrator (S0) writes the full delegating impls once, each method calling a
  module function. Workers fill in those module functions (plus their C++ file pair and their
  own bridge file). The stub signatures **are** the contract.
- `build.rs` globs `src/*.cpp` and `src/bridge/*.rs`, so adding files never edits shared paths.

## S0 — integrator refactor (must land before workers start)

1. Move the trait impls out of `src/lib.rs` into `src/runtime_impl.rs` and `src/display_impl.rs`,
   keeping `lib.rs` as glue (`mod` list, statics, creation helpers, `INVOKE_SYSTEM_SCRIPT`).
2. Split the bridge: `src/bridge/{window,webview,screen,cookies,handlers,runtime}.rs`, each its
   own `#[cxx::bridge]` module with its own generated header. Workers own one bridge file each.
3. `build.rs`: glob `src/*.cpp` (sorted) and `src/bridge/*.rs`; no per-file lists.
4. Create the worker module files with the exact signatures and `unimplemented!()`-free stubs:
   every stub returns `Option`/`bool` "unknown" rather than a fabricated value, and the impls map
   "unknown" to `Err(Error::…)` — no lying getters survive S0.
5. Verify `cargo build -p tauriqt` + the M1 smoke still pass, commit.

## Work graph

```mermaid
graph LR
    S0["S0 integrator<br/>module + bridge split<br/>freeze stub contracts"]
    W1["W1 window ops<br/>geometry · state · fullscreen<br/>icon · constraints · drag"]
    W2["W2 screen ops<br/>QScreen monitors · cursor · DPI"]
    W3["W3 webview ops<br/>geometry · focus · print<br/>zoom · clear data"]
    W6["W6 runtime handles<br/>raw handles · proxy<br/>request_exit · run_iteration"]
    W7["W7 app + demo page<br/>exercise every new API"]
    DEF["deferred<br/>W4 cookies (no-op)<br/>W5 tauri handlers (critical)"]
    I["I integrator<br/>merge · one build · strip stubs"]
    V["V GUI verification<br/>checklist run"]

    S0 --> W1
    S0 --> W2
    S0 --> W3
    S0 --> W6
    S0 --> W7
    S0 -.-> DEF
    W2 -.->|"Monitor providers (stubbed until merged)"| W1
    W2 -.->|"Monitor providers"| W6
    W1 --> I
    W2 --> I
    W3 --> I
    W6 --> I
    W7 --> I
    DEF --> I
    I --> V
```

Dashed edges are interface-only: the providers are stubbed in S0, so no worker ever waits.

## Workers

| Worker | Owns (only these files) | Delivers | Acceptance |
|---|---|---|---|
| W1 | `src/ops/window.rs`, `qt_window.cpp/.hpp`, `src/bridge/window.rs` | real geometry getters, `title`, `is_*` from Qt state, `set_fullscreen`, `minimize/unminimize`, min/max size + constraints, `set_icon`, `set_focus`, `start_dragging`, `start_resize_dragging` | builds; each method either reads Qt or returns `Err` |
| W2 | `src/ops/screen.rs`, `qt_screen.cpp/.hpp`, `src/bridge/screen.rs` | `Monitor` list/primary/from-point (name, size, position, work area, scale), `cursor_position`, `QCursor` set/pos, window scale factor | builds; monitor values match `qscreen` output |
| W3 | `src/ops/webview.rs`, `qt_webview.cpp` (own sections only) | webview geometry getters, `set_size/set_position/set_focus`, `eval_script_with_callback` (runJavaScript callback → Rust), `clear_all_browsing_data`, print/zoom/devtools polish | builds; callback returns a JSON string to Rust |
| ~~W4~~ | **deferred (no-op)** | cookies stay explicit `Err` for now; see "W4 design note" below | – |
| ~~W5~~ | **deferred (critical)** | wiring tauri's `PendingWebview` handlers changes navigation/new-window/link semantics; done by the integrator, alone, after the rest lands | – |
| W6 | `src/ops/runtime.rs`, `qt_runtime.cpp/.hpp`, `src/bridge/runtime.rs` | `window_handle` + `display_handle` (X11 `QX11Application` / Wayland `QWaylandApplication`), `EventLoopProxy::send_event`, `request_exit`, `run_iteration` | builds; proxy event reaches the run callback |
| W7 | `tauriqt/`, `dist/`, `PLAN.md`, `RUNTIME-TRAITS.md` | demo page exercising every new API + doc updates written from the merged result | builds; page shows one line per API |

## Rules given to every worker

1. `isolated: true` — work in your own worktree, commit there, never touch another worker's files.
2. Owned files only; if you need a change in `lib.rs`, `runtime_impl.rs`, `build.rs`,
   `Cargo.toml` or another worker's bridge file, report the exact change instead of making it.
3. Do not run `cargo` more than once, at the end of your work (one build + optional run).
4. Keep `unimplemented!()`/fabricated values out: unknown state is `Err(Error::…)`, never a guess.
5. Report: files, exact Qt APIs used, what you verified, what you could not.

## Integration (I do this)

1. Apply all worktrees, resolve the (expected: zero) overlaps on shared files.
2. One build; fix integration errors centrally rather than sending workers back.
3. Run the GUI checklist: window geometry/state, monitors, cookies, page-load/title events,
   proxy event, `invoke()` still green, `asset://` fetch, devtools open/close, print dialog.
4. Strip any remaining stubs that return fabricated values; update docs; commit.

## W4 design note (deferred)

- Writes are easy (`setCookie`/`deleteCookie` are fire-and-forget); the read path is the problem.
- `QWebEngineCookieStore::loadAllCookies()` has **no completion signal** — verified in the Qt 6.11.2
  header on disk and in the Qt dev branch; consumers only see a stream of `cookieAdded` signals.
  (A blog page claiming `loadAllCookiesFinished()`/`allCookies()` exists is wrong: `allCookies()` is
  `QNetworkCookieJar`, a different class.)
- Options when we pick it up: (A) writes only, reads `Err`; (C/D) subscribe to
  `cookieAdded`/`cookieRemoved` per profile and answer reads from a Rust mirror — no blocking, no
  re-entrancy, "observed snapshot" semantics; (B) nested `QEventLoop` with a settle timeout plus a
  re-entrancy guard that refuses to run inside a scheme-request callback.
- Preferred: **D** (mirror for reads, direct calls for writes). Note Qt loads the store
  automatically on first navigation, so the mirror fills without an explicit `loadAllCookies()`.

## Risks

- **Qt/Wayland semantics**: absolute positioning, `always_on_top`, `skip_taskbar` are not
  expressible on Wayland — W1 must return `Err` there, not fake success.
- **Cookies**: `QWebEngineCookieStore` is callback-only; blocking on the GUI thread deadlocks if
  the callback needs the GUI thread — W4 must pump a nested `QEventLoop` with a timeout.
- **Handler wiring (W5)** touches tauri's semantics (navigation deny, new-window allow) — a wrong
  default silently breaks links; W5 must default to "allow" and match wry's behaviour.
- **EventLoopProxy (W6)**: must be safe from any thread; queue + `invokeMethod(QueuedConnection)`.
- **Bridge growth**: 6 bridge files mean 6 generated headers; build.rs globbing keeps this free,
  but each worker must include only its own header in C++.
