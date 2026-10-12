<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

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
    W4["W4 cookie mirror<br/>cookieAdded/cookieRemoved<br/>Rust-side snapshot"]
    DEF["deferred<br/>W5 tauri handlers (critical)"]
    I["I integrator<br/>merge · one build · strip stubs"]
    V["V GUI verification<br/>checklist run"]

    S0 --> W1
    S0 --> W2
    S0 --> W3
    S0 --> W6
    S0 --> W7
    S0 --> W4
    S0 -.-> DEF
    W2 -.->|"Monitor providers (stubbed until merged)"| W1
    W2 -.->|"Monitor providers"| W6
    W1 --> I
    W2 --> I
    W3 --> I
    W6 --> I
    W4 --> I
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
| W4 | `src/ops/cookies.rs`, `src/qt_cookies.*`, `src/bridge/cookies.rs` | **cookie mirror**: subscribe to `cookieAdded`/`cookieRemoved`, answer `cookies()`/`cookies_for_url()` from the copy, writes via `setCookie`/`deleteCookie` + optimistic update | builds; JS↔Rust cookie round-trip in the demo |
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

## W4 decision (cookie mirror)

Chosen: **keep our own copy**. `QWebEngineCookieStore` can only enumerate asynchronously
(`loadAllCookies()` emits one `cookieAdded` per cookie) and **never signals completion** —
verified in the Qt 6.11.2 header on disk and in the Qt dev branch. `WebviewDispatch::cookies()`
is synchronous and our scheme handler already runs synchronously on the GUI thread, so blocking
is not acceptable.

Rejected alternatives:
- **Parse the Chromium `Cookies` SQLite file ourselves**: values can be `v10`/`v11` encrypted
  (v11 needs the user's GNOME Keyring/KWallet secret; ≥ schema v24 prefixes a `SHA-256(host_key)`
  digest), the file lags the live in-memory store (batched flushes, `SQLITE_BUSY`), and
  `PersistentCookiesPolicy` can mean session cookies are never written at all — a silently
  wrong list. Only appropriate for offline forensics tools.
- **Nested `QEventLoop` with a settle timer**: would run inside the synchronous `ipc://`
  scheme callback (re-entrancy, deadlock against the pending job) and a timeout can return a
  truncated list.
- **CDP `Network.getAllCookies`**: the only route to an authoritative enumeration with a real
  completion message, but needs a WebSocket client and an open local debug port. Kept in
  reserve (would also add console/network observability).

Mirror semantics to document: it is an **observed snapshot** — cookies written before the store
was watched appear only after the initial load/navigation; expiry-only changes may not emit an
event; CHIPS (`partitioned`) has no Qt equivalent.


## Testing track (WebDriver via chromedriver)

Goal: automate testing of apps running on this runtime with **Playwright**, and ultimately run
third-party Tauri app suites (Readest) against it.

| Slice | What | Status |
|---|---|---|
| T1 | **DONE — CDP measured + Playwright unblocked.** 62 commands / 96 passive events probed: 49 result, 9 protocol errors, zero timeouts. **Playwright 1.64 attaches** through `tools/cdp-probe/mitm.mjs` with **one stub** (`Browser.setDownloadBehavior -> {}`) plus three page-session rewrites (`Storage.getCookies -> Network.getAllCookies`, `Storage.setCookies -> N× Network.setCookie`, `Storage.clearCookies -> Network.clearBrowserCookies`) — **22/23 smoke steps pass** (title, evaluate, `awaitPromise` Tauri `invoke`, `page.click`, `page.screenshot` 210 KB PNG, `$eval`, cookies, viewport, console+pageerror events). Only failure: `context.newPage()` (`Target.createTarget` unsupported — no second webview). Structured caveats: (a) browser endpoint ≠ superset of a page session — `Network.enable`/`getAllCookies` are `-32601` on `/devtools/browser/<uuid>` and must be routed through a page session; (b) CDP cannot set cookies for `tauri://` origins (Chromium rejects non-http(s) schemes, `-32602`) → cookie tests must go through our runtime API or an http(s) origin; (c) `Browser.getVersion` product is `tauri-runtime-qt/` (Playwright tolerates, ChromeDriver does not → T1b rewrites it). Deliverables: `tools/cdp-probe/` (`probe.mjs`, `cdp-lib.mjs`, `mitm.mjs`, `playwright-attach.mjs`, `stubs.example.json`, raw transcripts), `docs/cdp-support-matrix.md`. | done |
| T1b | **DONE — chromedriver attaches.** Bare attach fails only because QtWebEngine publishes its CDP product string as `qApp->applicationName() "/" applicationVersion()` = `tauri-runtime-qt/` (empty version), which chromedriver can't parse; a 3-line rewrite of `/json/version` to a Chrome version whose **major matches the chromedriver** makes the session succeed: **33/39 WebDriver endpoints pass**, including `execute/sync|async` calling our Tauri `invoke()` (`Hello, wd-sync! — from linux`), element find/text/click/keys, `Input.dispatch*` (delivered with `isTrusted=true`), and a real screenshot (PNG 1277x1562). Only CDP method Qt lacks: **`Browser.getWindowForTarget`** → `window/rect` 404; plus ChromeDriver-side cookie-domain validation (`POST /cookie` 400) and legacy JSONWP endpoints. Details: `docs/webdriver-attach-findings.md`, harness in `tools/webdriver-probe/`. |
| T2 | **Wire the harnesses into `scripts/`.** Two proven paths, both to be wrapped: (a) **Playwright + MITM** for our own rich UI specs (`scripts/playwright-qt.sh`: app with CDP port → `mitm.mjs` → `chromium.connectOverCDP('http://127.0.0.1:<port>')`), (b) **chromedriver + `/json/version` fix-up** for WebDriver clients (WDIO, `@vitest/browser-webdriverio`, i.e. third-party suites). Version pairing decision still open: test-only fix-up proxy vs matching chromedriver major. | planned |
| T3 | **Window-op bridge only** (input/screenshots turned out to work over CDP, so this is now narrowly scoped): `window/rect` and friends need `Browser.getWindowForTarget`, which Qt lacks — serve those from a test-control channel in the app (JSON-RPC over a local socket mapped onto `ops::window`), so WebDriver window commands and CDP's missing Browser calls have a real implementation. | planned |
| T4 | **Run third-party suites**: Readest's `*.tauri.test.ts` (`@vitest/browser-webdriverio`) and WDIO E2E tiers against our WebDriver endpoint (standalone or `driverProvider`) — plus our own smoke specs, re-run on wry (`--features wry`) for engine parity. | planned |

Measured limits (T1b): Qt's CDP supports `Runtime.evaluate`, `DOM.*`, element queries via
ChromeDriver's own JS, `Network.getCookies`, **`Input.dispatchKeyEvent`/`dispatchMouseEvent`
(delivered with `isTrusted=true`)**, and **`Page.captureScreenshot`** (real PNG). What Qt does not
wire is browser-level: `Browser.getWindowForTarget` (→ WebDriver `window/rect` 404) and
`Browser.setDownloadBehavior` (which is also why Playwright's `connectOverCDP` aborts —
playwright#36961, closed not-planned; irrelevant now that we use WebDriver + chromedriver).
ChromeDriver-side, cookie writes fail its own domain validation (`POST /cookie` 400) — set cookies
via CDP `Network.setCookie` or through our cookie mirror instead. Un-addable and un-fakeable:
tracing, GPU state, browser-context management.


## Wayland-first CI harness (decision)

X11 is deprecated (RHEL) and our users' sessions are Wayland, so **Wayland is the primary
target**; X11 gets one small guard job only because our window ops branch on
`QGuiApplication::platformName()` and that branch would otherwise rot.

Everything comes from **Nix** — no runner-provided packages (no `apt-get`, no `actions/setup-*`).
Even on an Ubuntu-based runner we bring sway/grim/cage/weston/dbus/Xvfb/node/chromedriver ourselves
via the flake (`devShells.<system>.ci`) or `nix shell`.

### Tiers
| Tier | What | When |
|---|---|---|
| A | **Headless sway sessions** (`WLR_BACKENDS=headless WLR_RENDERER=pixman`), one private `XDG_RUNTIME_DIR` + `SWAYSOCK` per session → a dozen in parallel on one runner. `xwayland disable` guarantees Wayland-native coverage. | per-PR |
| B | **VM + KVM** `nixosTest` (hermetic: pinned NixOS, kernel/net isolation). KVM is available on GitHub amd64 runners via the Determinate Nix installer (GitHub's own docs still call nested virt unsupported); TCG fallback keeps it runnable. The test driver also gained a **systemd-nspawn container backend** for non-GUI tiers (its docs note: no GUI/setuid in the sandbox). | nightly / release |
| C | One tiny xcb (Xvfb) job | per-PR |

### Frozen session interface
```
scripts/sway-session.sh start [--outputs N] [--scale S] [--xwayland] [--json]
scripts/sway-session.sh stop <session-id|--all>
scripts/sway-session.sh exec <session-id> -- <cmd...>
scripts/wayland-smoke.sh [--timeout S]
scripts/sway-oracle.sh
```

### Why the compositor is our oracle
Wayland deliberately has no toplevel position: `xdg-shell` configure carries size + states only. So
`inner_position`/`outer_position`/`set_position` cannot report the *real* placement and instead
answer with Qt's record of what the app asked for (a client that refused the call outright broke
`tauri-plugin-window-state` on Wayland, so the `Err` this plan originally specified is gone — see
`docs/wayland-ci.md`). The *compositor* knows the real rect and state, so `swaymsg -t get_tree`
gives us an external oracle for the difference, and the ability to drive state changes
(`fullscreen enable`, `focus`, `move`) that our app must then reflect. Protocol notes for future
runtime work: `xdg-activation-v1` (would make `set_focus` reliable without a token),
`xdg-toplevel-icon-v1` (would make `set_icon` real), `wlr-foreign-toplevel-management` (state +
`set_rectangle` hints, still no position read).

## Risks

- **Qt/Wayland semantics**: `always_on_top` and `skip_taskbar` are not expressible on Wayland — W1
  must return `Err` there, not fake success. Absolute positioning is recorded rather than refused
  (see above), because callers treat a position as always available.
- **Cookies**: `QWebEngineCookieStore` is callback-only; blocking on the GUI thread deadlocks if
  the callback needs the GUI thread — W4 must pump a nested `QEventLoop` with a timeout.
- **Handler wiring (W5)** touches tauri's semantics (navigation deny, new-window allow) — a wrong
  default silently breaks links; W5 must default to "allow" and match wry's behaviour.
- **EventLoopProxy (W6)**: must be safe from any thread; queue + `invokeMethod(QueuedConnection)`.
- **Bridge growth**: 6 bridge files mean 6 generated headers; build.rs globbing keeps this free,
  but each worker must include only its own header in C++.

## Later rounds (same day)

| Worker | Task | Outcome |
|---|---|---|
| CA1 | extract the Qt-free logic into `tauri-runtime-qt-core` | new crate (no Qt, no `cxx`, no build script), runtime delegates to it, **65 unit tests** pass; `tauriqt` build, smoke and the packaged app unchanged |
| CB1 | merged llvm coverage pipeline | `scripts/coverage.sh` + `just coverage` + a CI job: instrumented build → harness runs (`TAURIQT_EXIT_AFTER_MS` for a clean profile flush) → `llvm-profdata` merge → one lcov for Rust **and** C++ → genhtml + ReportGenerator (`Html;Badges`). Measured: lines 47.9 % (1962/4095), functions 45.8 %, branches 27.2 % |
| CO1 | why the compositor never saw a window | **not a runtime bug**: the nix *build sandbox* has no `/dev/dri`, so QtWayland reports `EGL not available`, the `QQuickWidget`-backed top-level never commits a `wl_buffer` and sway never maps it. The oracle now skips those assertions fail-closed (quoting the evidence) and `ORACLE_REQUIRE_MAPPING=1` forbids skipping; on a host with DRM the same harness maps the window (36 passed / 1 failed, the failure being `app_id`) |
| — | third-party app tier | `integrations/` runs other people's apps: Readest boots on this runtime, `invoke()` reaches its Rust, its own 183-test suite passes. Its WebDriver-driven e2e cannot run — QtWebEngine has no `Target.createTarget` (docs/webdriver-attach-findings.md) |
