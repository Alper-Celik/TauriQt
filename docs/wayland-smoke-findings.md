<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT -->

# Headless Wayland smoke — findings

What the Wayland-first harness (`scripts/sway-session.sh`, `scripts/sway-ci.conf`,
`scripts/wayland-smoke.sh`) is, what it measured, and what it proved about running
the Qt runtime under a headless Wayland compositor.

All measurements: 2026-10-11, NixOS `aarch64-linux`, Qt **6.11.2**
(`qtwebengine 6.11.2`), sway **1.12** (wlroots **0.20.2**), grim **1.5.0**, all from
nixpkgs; compositor on `WLR_BACKENDS=headless` with `WLR_RENDERER=pixman` (pure CPU
rasterisation, no GPU, no DRM node). Runs happened inside the agent firejail on a
machine whose own desktop session is X11/xcb.

## Verdicts

1. **QtWebEngine renders under headless sway + pixman — yes, out of the box.**
   The demo page loads, its JavaScript runs, `document.title` events reach the
   runtime, all Tauri commands answer, and the window is genuinely painted
   (`grim` capture: 545 distinct colours against a 1-colour empty canvas).
   The minimal flag set is **`--no-sandbox` and nothing else** — no `--disable-gpu`,
   no `--use-gl=…`, no `--in-process-gpu`, no ozone switch. In this firejail the
   page even renders with *no* Chromium flags at all (see the matrix below); the
   harness keeps `--no-sandbox` because the repo's `scripts/smoke.sh` does and
   because it removes Chromium's setuid/namespace sandbox as a failure mode.
2. **Screenshots work — yes.** `grim` (wlr-screencopy) captures the headless pixman
   outputs fine; *not* a "screencopy unsupported" situation. Captures are of the
   composited output, opaque, correct size, and show the client's pixels.
3. **The runtime really is on the Wayland platform**: `platform=wayland`, a Wayland
   display handle and a Wayland surface handle, and `Err(Unavailable)` for handles
   that do not exist (quoted below).
4. **The shipped demo page's verdict under Wayland is `DEMO_PARTIAL`, for a correct
   reason.** The page treats every `X=ERR(…)` field of a result as a failure, and
   under Wayland `inner_position`/`outer_position` *must* fail (a compositor owns
   placement). `wayland-smoke.sh` therefore accepts a `DEMO_PARTIAL` whose entries
   are **only** those two Wayland position errors and nothing else, and
   `--strict-demo-ok` demands the literal `DEMO_OK` once the page is made
   platform-aware. This is the only deviation from `scripts/smoke.sh`'s assertion
   list, and it is deliberate: the alternative is a check that fails for a correct
   reason on every Wayland run.

5. **A Nix *build sandbox* cannot map a toplevel at all — and that is the
   environment, not the runtime.** The flake checks run in a sandbox with no
   `/dev/dri`; without EGL Qt cannot composite the QtWebEngine widget tree
   (`EGL not available` → `Failed to create QRhi for QBackingStoreRhiSupport`), so
   the client never commits a `wl_buffer` and the compositor legitimately sees no
   view. The harnesses keep their compositor/paint assertions and *skip* them only
   under that quoted proof (fail-closed); `--require-mapping` on the oracle forbids
   skipping for machines that can map. Full evidence: §5 below.

## Files

| file | role |
| --- | --- |
| `scripts/sway-session.sh` | session manager: `start` / `stop` / `exec` / `list` / `store` |
| `scripts/sway-ci.conf` | sway config: `xwayland disable`, no bars, solid-colour canvas, no decorations/gaps |
| `scripts/wayland-smoke.sh` | smoke test: boots a session, runs the app through the Wayland platform plugin, asserts traces, optional `grim` capture |

Session layout: each session gets its own `0700` `XDG_RUNTIME_DIR` at
`${TMPDIR:-/tmp}/tauriqt-sway/run/sway-XXXXXXXX/` (holding `sway.conf`, `sway.log`,
`sway.pid`, the `wayland-1` socket and the sway IPC socket), plus one small
`${TMPDIR:-/tmp}/tauriqt-sway/sway-XXXXXXXX.session` state file the other commands
read. `stop` kills sway's process group (via `swaymsg exit`, then SIGTERM, then
SIGKILL), reaps any client still carrying the session's `XDG_RUNTIME_DIR` in its
environment, drains the group and removes the runtime dir; it is idempotent, and
`stop --all` sweeps orphaned runtime dirs whose state file went missing.

Binary discovery order (no runner-provided packages required): `SWAY_BIN`/`GRIM_BIN`
→ `TAURIQT_SWAY_PKG`/`TAURIQT_GRIM_PKG` → `PATH` → a cached store path →
`nix build nixpkgs#sway|#grim`. `nix` is never called when any earlier source hits.

## Evidence

### 1. Composing a session

```
$ scripts/sway-session.sh start --json
{"session":"sway-i1016dvK","XDG_RUNTIME_DIR":"/tmp/tauriqt-sway/run/sway-i1016dvK",
 "WAYLAND_DISPLAY":"wayland-1","SWAYSOCK":"…/sway-ipc.sock","pid":119524,
 "outputs":1,"scale":1,"resolution":"1280x720","xwayland":false,"display":"", …}
```

Four sessions started in parallel (`--outputs 1..4`), each in its own runtime dir —
all of them report their own `wayland-1` socket inside that dir and the requested
output count, so a dozen sessions per machine is a resource question, not a
correctness one:

```
sway-i1016dvK | 1 output  | wayland-1 | /tmp/tauriqt-sway/run/sway-i1016dvK
sway-ute7QsvK | 2 outputs | wayland-1 | /tmp/tauriqt-sway/run/sway-ute7QsvK
sway-MRekBTfC | 3 outputs | wayland-1 | /tmp/tauriqt-sway/run/sway-MRekBTfC
sway-pPNPKYbE | 4 outputs | wayland-1 | /tmp/tauriqt-sway/run/sway-pPNPKYbE
```

Outputs are driven by `WLR_HEADLESS_OUTPUTS` plus generated `output HEADLESS-N
mode/resolution/scale` lines in the per-session config (sway applies them when the
output appears), with `swaymsg create_output` as fallback:

```
$ scripts/sway-session.sh start --outputs 3 --scale 2 --resolution 800x600
$ swaymsg -t get_outputs
[('HEADLESS-3', 400, 300, 2.0, []), ('HEADLESS-2', 400, 300, 2.0, []), ('HEADLESS-1', 400, 300, 2.0, [])]
```

Observation worth knowing for screen-op coverage: sway reports that output's
*logical* rect as `640x360` for a `1280x720` mode at `scale 2`, while Qt's screens
report the physical size with the scale factor (`size=1280x720 … scale=2.0000`):

```
[tauriqt-demo] monitors: available_monitors=2 | #0 name=Some("HEADLESS-2") pos=0,0 size=1280x720 work=1280x720 scale=2.0000 | #1 name=Some("HEADLESS-1") pos=1280,0 size=1280x720 work=1280x720 scale=2.0000 …
```

**XWayland is not exercisable in this sandbox**: `/tmp/.X11-unix` is mounted
read-only, so wlroots cannot bind its socket and sway logs

```
… [wlr] [xwayland/sockets.c:47] Failed to bind socket /tmp/.X11-unix/X21: Read-only file system
… [wlr] [xwayland/sockets.c:200] No display available in the first 33
… [sway/server.c:766] Failed to start Xwayland
```

`start --xwayland` detects that, warns `sway could not start Xwayland …; DISPLAY
stays unset`, keeps the Wayland session usable and leaves `DISPLAY` out of the
printed environment. On a normal host (writable `/tmp/.X11-unix`) the same code
path finds the display number from the `X<n>` lock file whose process group is the
session's; that end-to-end path is therefore **unverified here**.

### 2. Rendering verdict (the app under Wayland)

```
$ scripts/wayland-smoke.sh
==> starting headless sway session (outputs=1 scale=1)
    session sway-zvgOOTty  wayland=wayland-1  runtime=/tmp/tauriqt-sway/run/sway-zvgOOTty
==> running ./target/debug/tauriqt for up to 60s
    chromium flags: --no-sandbox
==> 11 expectations passed (run stopped as soon as it reached its verdict)
```

Trace excerpt (app log, Wayland platform plugin):

```
[tauri-runtime-qt] run: emitting Ready
[tauri-runtime-qt] window 'main' created (native 1)
[tauri-runtime-qt] webview 'main' created in window 1 (native 1), url tauri://localhost
[tauri-runtime-qt] run: entering Qt event loop
[tauri-runtime-qt] tauri GET tauri://localhost/ -> 200 (4256 body bytes)
[tauri-runtime-qt] webview 1 event 4: tauri://localhost
[tauri-runtime-qt] webview 1 event 4: TauriQt — runtime demo
[tauri-runtime-qt] ipc POST ipc://localhost/greet -> 200 (27 body bytes)
[tauriqt-demo] greet: Hello, Qt! — from linux
[tauri-runtime-qt] webview 1 event 4: DEMO_PARTIAL: win_info -> inner_position=ERR(…) | win_info_after_fullscreen -> …
```

`webview 1 event 4:` lines are `document.title` changes, i.e. **the renderer
process ran the page's JavaScript**; the `ipc POST … -> 200` lines are the full
command round-trip (`greet`, `win_info`, `monitors`, `set_title`, `set_fullscreen`,
`report`) — 15 of them in a complete run. `run: entering Qt event loop` and
`webview 'main' created … url tauri://localhost` show the Qt/Wayland side is up
before any of that.

Note for anyone asserting on the verdict: the runtime truncates the title *event*
line at ~768 characters (the excerpt above ends in `…`), while the demo also reports
the same string through `report`, so `[tauriqt-demo] title=DEMO_…` is the complete
827-character line. `wayland-smoke.sh` reads the verdict from that `title=` line.

No GPU is available or needed: the compositor is pixman-only, and the app's
Chromium side falls back by itself. The only noise is harmless:

```
MESA-EGL: warning: failed to get driver name for fd -1
MESA: error: Opening /dev/dri/card2 failed: Permission denied      # firejail blocks /dev/dri
[115974:116015:…:ERROR:…/chromium/dbus/bus.cc:408] Failed to connect to the bus: …/firejail/mnt/dbus/system: Permission denied
[118524:7:…:ERROR:…/gpu/ipc/client/command_buffer_proxy_impl.cc:127] ContextResult::kTransientFailure: Failed to send GpuControl.CreateCommandBuffer.
```

Chromium flag matrix (same session parameters, app started by hand):

| `QTWEBENGINE_CHROMIUM_FLAGS` | page loads (`tauri GET … -> 200`) | JS runs | demo verdict reached | paint |
| --- | --- | --- | --- | --- |
| `--no-sandbox` (harness default) | yes | yes | `DEMO_PARTIAL` | yes (545 colours) |
| *(empty — no flags at all)* | yes | yes | `DEMO_PARTIAL` | not captured |

So `--no-sandbox` is **not** required for rendering on this machine; it is kept
because the firejail setup in this repo expects it and because it removes the
Chromium sandbox as a variable. `--disable-gpu`, `--use-gl=…` and
`--in-process-gpu` were never needed. `--disable-dev-shm-usage` is added
automatically only when `/dev/shm` is missing or unwritable (relevant for a Nix
build sandbox), and the effective flag string is printed on every run.

### 3. Screenshot verdict

`grim` works against the headless pixman compositor (wlr-screencopy is supported by
the headless backend; **there is no error to report** — captures succeed):

```
$ scripts/wayland-smoke.sh --screenshot /tmp/wl-shot-app.png --skip-build
==> screenshots
  ok   : wl-shot-app.png: 1280x720, 141386 bytes
  ok   : capture differs from the empty-session baseline (baseline wl-shot-app.baseline.png:
         1280x720, 2759 bytes) — the client painted
```

Pixel analysis of the two captures (PNG decoded, every 4th pixel sampled):

```
baseline (empty session): 1280x720, distinct colours = 1   (#000000 everywhere, 0.0% non-black)
app capture (demo page)  : 1280x720, distinct colours = 545 (99.7% non-black)
    top colours: #ffffff=53052, #b3261e=381, #000000=160, #f8eae9=95
```

`#ffffff` is the page background, `#000000` the text, and `#b3261e` is the demo
page's `.bad` (FAIL) colour — i.e. the capture contains the *rendered page with its
`DEMO_PARTIAL` summary*, not just an empty window. The empty-session baseline is a
single black colour, so "did the client paint?" is a byte-comparison of two
captures, which is what `--screenshot` asserts (no image library needed).

Other `grim` behaviour observed: `grim -o HEADLESS-1 file.png` captures one output
(1280x720); `grim file.png` with no output selection captures the whole layout
(e.g. 2400x600 for three 800x600 outputs); an unknown output fails loudly with
`unknown output 'NOPE'` and exit 1.

### 4. Wayland-specific handles and expected `Err`s

`TAURIQT_PROXY_DEBUG=1` (via `scripts/wayland-smoke.sh --proxy-debug`) prints the
native state and the raw-handle probes:

```
[tauri-runtime-qt] proxy-debug: native state: platform=wayland kind=3 display=0x555555e95510 topLevelWidgets=1 [winId=0x555555f954c0 visible=1 title="TauriQt"]
[tauri-runtime-qt] proxy-debug: display_handle() = Ok(DisplayHandle(Wayland(WaylandDisplayHandle { display: 0x555555e95510 })))
[tauri-runtime-qt] proxy-debug: window_handle(u64::MAX) = Err(Unavailable)
[tauri-runtime-qt] proxy-debug: window_handle(1) = WindowHandle(Wayland(WaylandWindowHandle { surface: 0x555555f954c0 }))
[tauri-runtime-qt] proxy-debug: sink received event #1: proxy-probe-1   (#2, #3 likewise)
[tauri-runtime-qt] proxy-debug: worker requests exit(0)
```

The probe run exits by itself with status 0, which the harness asserts.

The compositor-owned window state that Wayland makes unavailable shows up as
`Err` in the demo's own output (`win_info` reports each field rather than failing):

```
[tauriqt-demo] win_info: inner_size=1280x720; outer_size=1280x720;
  inner_position=ERR(runtime error: failed to create webview: tauri-runtime-qt: the window
  position is unavailable: the 'wayland' Qt platform does not report top-level window positions);
  outer_position=ERR(… the window frame position is unavailable: the 'wayland' Qt platform does not
  report top-level window positions); scale_factor=1.0000; title=TauriQt; is_fullscreen=false;
  is_minimized=false; is_maximized=false; is_focused=true; … is_always_on_top=false; …
```

`inner_position`/`outer_position` are the only such errors the shipped demo
exercises, and they are the ones the harness accepts (they also name the platform,
which is how the harness proves there was no X11 fallback). The *setters* that
Wayland cannot honour take the same branch in the runtime and are reachable only
from code the demo does not call, so their messages below are read from the source,
not observed at runtime (`tauri-runtime-qt/src/ops/window.rs`, `unavailable()` at
`:45`):

| call | message (every `Err` is `Error::CreateWebview`) |
| --- | --- |
| `set_position` (`:430`) | `tauri-runtime-qt: absolute window positioning is unavailable: the 'wayland' Qt platform does not let clients place top-level windows` |
| `set_always_on_top` (`:406`) | `tauri-runtime-qt: always-on-top is unavailable: the 'wayland' Qt platform has no on-top state (Wayland compositors ignore Qt::WindowStaysOnTopHint, QTBUG-73456)` |
| `set_always_on_bottom` (`:391`) | `tauri-runtime-qt: always-on-bottom is unavailable: the 'wayland' Qt platform has no on-bottom state (a Wayland compositor does not expose one)` |

### 5. Nix build sandbox: no `/dev/dri` ⇒ no EGL ⇒ no mapped toplevel

Measured inside the Nix build sandbox — the environment the flake checks actually
run in, and *not* the firejail the rest of this document was measured in: **it has
no `/dev/dri` at all** (the firejail has the device nodes and merely denies access
to them). That difference decides the whole picture.

The compositor there sees the client's toplevel but never a mapped view:

```
$ LD_LIBRARY_PATH= nix build .#checks.aarch64-linux.sway-oracle --no-link -L
  FAIL compositor sees no xdg_shell toplevel for the app
  info xdg_shell nodes   : 0
```

while sway's own debug log (the oracle runs sway with `-d`) shows the client *did*
create its toplevel — and the app is otherwise entirely healthy (`title=DEMO_OK`,
all 15 IPC commands answer 200, `win_info … is_visible=true`, monitors/scale
correct, `TAURIQT_PROXY_DEBUG` still reports `platform=wayland` with a real Wayland
surface handle):

```
00:00:00.608 [DEBUG] [sway/desktop/xdg_shell.c:559] New xdg_shell toplevel title='(null)' app_id='(null)'
```

Why nothing maps, in Qt's own words from that run (`qtbase 6.11.2`; requires
`QT_FORCE_STDERR_LOGGING=1`, see below — without it Qt logs through syslog, not
stderr, because the harness runs the app under `setsid` with stdio redirected):

```
qt.qpa.wayland: Available client buffer integrations: QList("wayland-egl")
qt.qpa.wayland: Initializing client buffer integration "wayland-egl"
qt.qpa.wayland: EGL not available
QRhiGles2: Failed to create temporary context
QRhiGles2: Failed to create context
Failed to create RHI for backend: OpenGL
qt.widgets.painting: Tree with root QQuickWidget(0x…) evaluates to flushing with QRhi
qt.widgets.painting: Recreating QWidgetWindow(0x…, name="mainWindow") with current type QSurface::RasterSurface to support QSurface::OpenGLSurface
Failed to create QRhi for QBackingStoreRhiSupport
qt.widgets.painting: Flushing QRegion(0,0 1024x768) of QWidget(0x…, name="main") to QWidgetWindow(0x…, name="mainWindow") using RHI
QQuickWidget: Attempted to render scene with no rhi
```

The chain, each step verified against the Qt sources:

1. `QWebEngineView` is a `QQuickWidget` — verified: `libQt6WebEngineWidgets.so.6.11.2`
   references `QQuickWidget::paintEvent`/`setContent`/`qt_metacast`/… — i.e. a
   render-to-texture child. `QWidgetRepaintManager` therefore flushes the *top-level
   window* through `QPlatformBackingStore::rhiFlush()` rather than the raster path,
   and Qt recreates the `QWidgetWindow` as an `OpenGLSurface` window
   (`qwidget.cpp` `q_evaluateRhiConfig*()`; `qwidgetrepaintmanager.cpp:1051`).
2. That path needs a QRhi with a GL/EGL backend. With no DRM device QtWayland's
   `wayland-egl` client buffer integration cannot initialise
   (`EGL not available`, `qwaylandeglclientbufferintegration.cpp`), so both
   `QRhi::create(OpenGLES2)` and `QBackingStoreRhiSupport::create()` fail.
3. `QBackingStoreDefaultCompositor::flush()` then returns
   `QPlatformBackingStore::FlushFailed` **silently** — its first statement is
   `if (!rhi || !swapchain) return QPlatformBackingStore::FlushFailed;`
   (`src/gui/painting/qbackingstoredefaultcompositor.cpp`) — so Qt never attaches a
   `wl_buffer`. Sway inserts a view into its tree only from `view_map`, i.e. on the
   first committed buffer (`sway/tree/view.c`, `sway/desktop/xdg_shell.c`), so
   `get_tree` correctly reports no toplevel at all.
4. `grim` agrees at the pixel level: with `--screenshot` that session's capture is
   byte-identical to the empty-session baseline (2759 bytes, one black colour, 0
   painted pixels), while the same command on a machine with a GL path reports
   `capture differs from the empty-session baseline … the client painted`
   (545 distinct colours).

So this is an **environment limit of a GL-less sandbox, not a runtime gap**: the
client creates its window, stays healthy and reports truthfully — it just cannot
composite anything into a surface. Nothing in the runtime can conjure up the missing
DRM device, and the same app + harness map (and paint) the window wherever EGL
exists.

#### What the checks do about it

The assertions are kept; only the *environment* is allowed to excuse them, and only
with the proof quoted above:

- `scripts/wayland-smoke.sh --screenshot` (what the flake check runs) keeps the
  paint assertion. A capture identical to the empty baseline is a **FAIL** — unless
  the same run proves it could not paint (no `/dev/dri`, plus Qt's
  `EGL not available` *and* `Failed to create QRhi for QBackingStoreRhiSupport`), in
  which case it prints a `SKIP` line quoting that evidence and reports
  `N expectations checked: M passed, K skipped`. Fail-closed: if the evidence does
  not hold, the old failure stands.
- `scripts/sway-oracle.sh` keeps every compositor assertion (rect, focused, app_id,
  fullscreen, move position, floating, title) and skips the compositor-visible half
  only under the same proof — quoting Qt's `EGL not available`, the failed QRhi, the
  client's toplevel from sway's log and `mapped xdg_shell views … (the client never
  committed a buffer)`. `--require-mapping` / `ORACLE_REQUIRE_MAPPING=1` forbids
  skipping, for machines that can map. Because the gate requires the client's
  toplevel to exist in sway's log, a genuine "the runtime stopped showing the
  window" regression still fails.
- Both harnesses now set `QT_FORCE_STDERR_LOGGING=1` (plus
  `QT_LOGGING_RULES=qt.widgets.painting=true`) for the app: Qt routes its log through
  syslog when it has no controlling TTY and stderr is not a terminal
  (`qtbase: shouldLogToStderr()`, `syslog_default_message_handler`), which had made
  every Qt-side message — including the ones above — invisible in the app log.

## Minimal environment

What the working setup actually needs (everything else is optional polish):

```
XDG_RUNTIME_DIR=<private 0700 dir>      # per session, ephemeral
WAYLAND_DISPLAY=wayland-1                # sway's socket name inside that dir
SWAYSOCK=<dir>/sway-ipc.sock             # the session manager fixes it
QT_QPA_PLATFORM=wayland                  # the app's platform plugin
QTWEBENGINE_CHROMIUM_FLAGS=--no-sandbox  # firejail convention; not strictly needed
# compositor: WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1
#             WLR_NO_HARDWARE_CURSORS=1 WLR_HEADLESS_OUTPUTS=<N>
# DISPLAY: unset (keeps Qt on Wayland instead of silently falling back to X11)
```

`sway-session.sh exec` unsets `DISPLAY` for the command it runs; `wayland-smoke.sh`
additionally gives the app a scratch `HOME`/`XDG_{DATA,CONFIG,CACHE,STATE}_HOME`
(so parallel runs cannot collide in a QtWebEngine profile) and unsets
`DBUS_SESSION_BUS_ADDRESS` for the app only — sway itself inherits whatever bus the
caller has, which is what a Nix check providing its own `dbus-run-session` needs.

## Using the harness (for the other workers)

```bash
# (a) start / stop a session (shell-exports form; stdout is `eval`-able)
eval "$(scripts/sway-session.sh start)"                    # or: --outputs 2 --scale 1
eval "$(scripts/sway-session.sh start --json)"             # JSON instead of exports
echo "$TAURIQT_SWAY_SESSION"                               # the session id
scripts/sway-session.sh list                               # sessions + liveness
scripts/sway-session.sh stop "$TAURIQT_SWAY_SESSION"       # one session
scripts/sway-session.sh stop --all                         # every session of this state dir (idempotent)

# (b) run anything inside a session (env + exit status pass through)
scripts/sway-session.sh exec "$TAURIQT_SWAY_SESSION" -- ./target/debug/tauriqt
scripts/sway-session.sh exec "$TAURIQT_SWAY_SESSION" -- \
    "$(scripts/sway-session.sh store grim)" /tmp/shot.png
scripts/sway-session.sh exec "$TAURIQT_SWAY_SESSION" -- \
    "$(scripts/sway-session.sh store sway swaymsg)" -t get_tree
```

Useful knobs: `TAURIQT_SWAY_STATE` (state dir, default `${TMPDIR:-/tmp}/tauriqt-sway`),
`TAURIQT_SWAY_BOOT_TIMEOUT` (default 25 s), `SWAY_BIN`/`GRIM_BIN`,
`TAURIQT_SWAY_PKG`/`TAURIQT_GRIM_PKG`, `TAURIQT_SWAY_KEEP=1` (keep runtime dirs on
`stop`). `wayland-smoke.sh` also takes `--timeout S`, `--outputs N`, `--scale S`,
`--screenshot PATH`, `--proxy-debug`, `--strict-demo-ok`, `--skip-build`,
`--keep-session`, and honours `TAURIQT_BIN`, `SMOKE_SKIP_BUILD`,
`SMOKE_TIMEOUT`/`WAYLAND_SMOKE_TIMEOUT`, `WAYLAND_SMOKE_HOME`.

## Reproducing these results

```bash
# workstation path (build + run inside the devShell, the repo's current idiom)
LD_LIBRARY_PATH='' nix develop -c sh -c 'QMAKE=$(which qmake); cargo build -p tauriqt'
scripts/wayland-smoke.sh                       # ~11-17 s including an incremental build
scripts/wayland-smoke.sh --skip-build          # ~5 s
scripts/wayland-smoke.sh --screenshot /tmp/shot.png --skip-build
scripts/wayland-smoke.sh --proxy-debug --skip-build
scripts/wayland-smoke.sh --strict-demo-ok --skip-build   # fails today on purpose
scripts/sway-session.sh stop --all             # idempotent; run it when in doubt

# prebuilt binary path (what the flake check uses: no nix call, no LD_LIBRARY_PATH)
TAURIQT_BIN=/nix/store/…-tauriqt/bin/tauriqt SMOKE_SKIP_BUILD=1 scripts/wayland-smoke.sh
```

Two consecutive runs, a `--screenshot` run, a `--proxy-debug` run, two *parallel*
runs and a `--outputs 2 --scale 2` run all exited 0 here; after them `ps` showed no
`sway`, `swaybg`, `tauriqt` or `QtWebEngineProcess` left, and `sway-session.sh stop
--all` stayed a no-op. `shellcheck -S warning` is clean on both scripts.

## Limits of this evidence

- Everything except §5 was measured in the agent firejail, *not* in the Nix build
  sandbox the checks run in; §5 is the one section measured inside the build, and it
  is where the "no toplevel" behaviour lives. The two environments differ in exactly
  one way that matters here: the build sandbox has no `/dev/dri` (and therefore no
  EGL/GL path), the firejail has the nodes but denies access to them.
- The compositor assertions in `scripts/sway-oracle.sh` and the paint assertion in
  `scripts/wayland-smoke.sh --screenshot` are therefore only *exercised* on a
  machine with a GL path (or a build sandbox given one); in the GL-less sandbox they
  report `SKIP` with the evidence. Their positive path is the one the earlier
  sections of this document record (37/0 and "the client painted"), not something
  reproducible in the sandbox.
- **`app_id` is empty when the window *does* map** (measured on the firejail, where
  every other compositor assertion passes): `swaymsg -t get_tree` reports
  `{"app_id":"", "name":"TauriQt — demo title", …}`. Qt derives the Wayland app-id
  from `QGuiApplication::desktopFileName()`, else from the *executable file path*
  (`qwaylandwindow.cpp`: `QFileInfo(applicationFilePath()).baseName()`, plus the
  reverse-domain prefix from `organizationDomain()`). The runtime hands Qt a
  fabricated `argv[0]` (`qt_app.cpp`: `static char argv0[] = "tauri-runtime-qt"`), so
  Qt cannot resolve an application file path and `set_app_id("")` goes out. Fixing it
  means passing the real argv through the cxx bridge (or calling
  `QGuiApplication::setDesktopFileName()`); until then the oracle's
  `compositor: app_id=…` assertion fails on machines that can map — the one gap this
  change does not close, and the reason the assertion is *skipped* (never *passed*)
  in the GL-less build sandbox.
- No GPU/EGL/GL path was exercised (pixman + headless by design); `--use-gl=…`
  settings are untested and unnecessary here.
- No input devices (`WLR_LIBINPUT_NO_DEVICES=1`), so pointer/keyboard-driven
  behaviour is out of scope for this harness.
- XWayland cannot start in this sandbox (read-only `/tmp/.X11-unix`), so
  `--xwayland`'s `DISPLAY` detection is unverified end-to-end.
- `cage`/`weston` were not tried; the harness is sway-only (`SWAY_BIN`,
  `SWAYSOCK`, `swaymsg`).
- The runtime's setters for placement/always-on-top/bottom are not invoked by the
  shipped demo, so their Wayland `Err` messages are quoted from source, not captured
  from a run (the getters and `win_info` fields *were* exercised).
- Until the demo page becomes platform-aware, `DEMO_OK` is unreachable under
  Wayland; `--strict-demo-ok` documents that as a failing check rather than hiding
  it.
