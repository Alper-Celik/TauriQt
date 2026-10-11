<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT -->

# Wayland CI: headless sway sessions as an external oracle

Date: 2026-10-11 · Machine: aarch64 NixOS, agent firejail sandbox
App: `tauriqt` (`target/debug/tauriqt`) · Runtime: `tauri-runtime-qt` on Qt 6.11.2 (Wayland QPA)
Harness: `scripts/sway-oracle.sh` · Session primitive: `scripts/sway-session.sh` (when present)

## Why a compositor

Under Wayland the runtime refuses the operations a compositor owns rather than
guessing: `set_always_on_top`/`set_always_on_bottom` return `Err`, because
xdg-shell gives a client no stacking request. Positions are answered, not
refused: `set_position` records the request (Qt's `move()` is a no-op for a
Wayland toplevel) and `inner_position`/`outer_position` report that record, so
callers that treat a position as always available — `tauri-plugin-window-state`,
whose startup restore `show()`s Readest's hidden main window — keep working. The
protocol still gives a client no way to *read* a toplevel's real position
afterwards, which is what the compositor oracle below is for. See
[protocol facts](#protocol-facts-and-sources).

The compositor, however, *does* know the truth. A headless **sway** session is
therefore two things at once:

1. a **driver** — it can focus, fullscreen and move our window even when our own
   code cannot; and
2. an **oracle** — `swaymsg -t get_tree` reports the real `rect`, `focused`,
   `fullscreen_mode`, `floating` and `app_id` for the toplevel, so our state
   reporting can be checked from the outside instead of trusted.

`scripts/sway-oracle.sh` starts three short sessions and asserts on both sides.
It prints `PASS`/`FAIL`/`SKIP` per assertion, exits non-zero on any failure, and
reaps every session it started. (The sample below is from a machine with a GL path;
in the GL-less build sandbox the compositor-visible assertions print `SKIP` with the
evidence — see `docs/wayland-smoke-findings.md` §5.)

```
$ scripts/sway-oracle.sh --no-build
== (a) state oracle: compositor drives, runtime reports ==
  PASS compositor: real window rect 0,25 1280x695
  PASS compositor: fullscreen enable -> fullscreen_mode=1
  PASS compositor: move position 300 200 -> rect at 300,225
== (b) positions: compositor rect vs the position we recorded ==
  compositor (swaymsg -t get_tree) rect : 300,225 1028x722
  runtime (win_info report)            : inner_position=0,0 outer_position=0,0
  PASS runtime reports the position Qt recorded for the app (0,0: the demo asks for none)
  PASS runtime answers position queries on wayland instead of the old Err
== (c) multi-output + scale ==
  runtime monitors: available_monitors=2 | ... #1 name=Some("HEADLESS-1") pos=2560,0 size=1280x720 scale=2.0000
== (d) X11-free proof ==
  PASS runtime: platform=wayland (no xcb fallback)
  assertions: 37 passed, 0 failed over 3 app run(s)
```

## Session isolation model

Each session is a private world, so any number can coexist on one machine:

| Concern            | Value                                                        |
| ------------------ | ------------------------------------------------------------ |
| Socket dir         | fresh `mktemp -d`, `chmod 700` → `XDG_RUNTIME_DIR`            |
| Wayland socket     | `wayland-N` created inside that dir (`WAYLAND_DISPLAY`)       |
| Compositor IPC     | `SWAYSOCK=$XDG_RUNTIME_DIR/sway-ipc.sock`                     |
| Backend            | `WLR_BACKENDS=headless` (wlroots headless backend, no seat)   |
| Renderer           | `WLR_RENDERER=pixman` (software; no GPU/DRM needed)           |
| Input devices      | `WLR_LIBINPUT_NO_DEVICES=1` (no libinput, no `/dev/input`)    |
| Outputs            | `WLR_HEADLESS_OUTPUTS=N` (default 1; each `HEADLESS-n`)       |
| X11                | `xwayland disable` in the config file                        |
| Qt platform        | `QT_QPA_PLATFORM=wayland` forced for the app                 |
| Chromium           | `QTWEBENGINE_CHROMIUM_FLAGS=--no-sandbox`                    |

The compositor is started with `setsid`, so it leads its own process group;
teardown is `kill -TERM -- -<pgid>` then `kill -KILL`, which takes sway *and*
everything it spawned. The runtime dir is `rm -rf`'d afterwards. `--keep` leaves
the sessions up for debugging and skips the leak assertion.

State that must outlive a session (the oracle's own logs, the app log) is written
under `${TMPDIR:-/tmp}/tauriqt-oracle/`, never inside `XDG_RUNTIME_DIR`.

## N sessions in parallel

Sessions do not share a single global resource: no display number, no fixed
socket path, no X authority, no seat. `WLR_HEADLESS_OUTPUTS` creates virtual
outputs in wlroots, so a session needs no kernel DRM device and no `/dev/dri`
access — important inside a sandbox. The only shared inputs are CPU and memory
(QtWebEngine is the heavy part), so parallelism is bounded by RAM, not by the
compositor. On CI, run each session with its own `XDG_RUNTIME_DIR` and give each
app a wall-clock timeout; `sway-oracle.sh` runs three back-to-back to keep the
default footprint small.

## What the compositor can and cannot oracle

| Property          | sway `get_tree` | our runtime        | Note                                                   |
| ----------------- | --------------- | ------------------ | ------------------------------------------------------ |
| window rect / pos | **yes** (real)  | recorded request   | compositor owns placement, and the protocol has no position read; see below |
| `focused`         | yes             | yes (`is_focused`) | compositor drives it                                   |
| fullscreen        | yes             | yes                | both directions: app `set_fullscreen`, `swaymsg fullscreen enable` |
| floating / tiled  | yes             | n/a                | compositor policy, not a client property               |
| title             | yes             | yes                | `xdg_toplevel.set_title` — oracle for `set_title`      |
| `app_id`          | yes             | n/a                | `xdg_toplevel.set_app_id`; here `tauriqt`              |
| always-on-top     | **no**          | `Err`              | no xdg-shell stacking request; Qt hint ignored         |
| global cursor pos | **no**          | `Err`              | clients only learn pointer position from their own events |

### Positions: the compositor's rect, and what we tell the app

`swaymsg -t get_tree` returns `rect {x,y,width,height}` for the toplevel. In a
headless session the app window lands at a real coordinate, and once floated and
`move position 300 200`'d the rect follows (`300,225 1028x722` here). The same
run's `win_info` report is `inner_position=0,0 outer_position=0,0`: the demo asks
for no position, and the runtime answers with Qt's record of the *request*
instead of refusing (`Err`) or echoing a rect a client cannot read. Both sides
are quoted by the harness, so the two facts are asserted together — a regression
either to the old position `Err` or to a fabricated non-zero value fails the
check.

The residual gap is documented rather than papered over: the runtime cannot
follow a compositor-driven move (`move position 300 200` above moved the rect, and
`outer_position` still reports the app's own record), because xdg-shell has no
request to read a toplevel's position. Applications that need the real geometry
must ask their own compositor integration, not the client API.

Even the closest thing Wayland has to a window list,
`wlr-foreign-toplevel-management`, cannot read geometry: it exposes `title`,
`app_id` and `state`, and its `set_rectangle` is a **hint the client sends to the
compositor** (used e.g. for minimizing), not a compositor→client query. There is
no request to read a toplevel's rect.

### Always-on-top: no

There is no xdg-shell request that maps to "stay above other windows"; stacking
is entirely the compositor's policy, and Qt's `Qt::WindowStaysOnTopHint` is
documented as ignored on the Wayland platform (`QTBUG-73456`). So the runtime
returns `Err` and *no* compositor query can act as a positive oracle for it.

## Screenshots: `grim` vs our own capture

- **`grim`** captures any output over `wlr-screencopy-unstable-v1`, which sway
  implements. Because the compositor has already composited the QtWebEngine
  surface, a grim grab is *pixel truth* including WebEngine content. It only
  needs `XDG_RUNTIME_DIR` + `WAYLAND_DISPLAY` of the session, so it works
  headless with no GPU. `grim -o HEADLESS-1 out.png`.
- **Our own capture**: the runtime has no screenshot API, and the app can't grab
  the composited window on Wayland (no `wl_surface` read-back for clients). The
  one in-process option is CDP (`Page.captureScreenshot`) against QtWebEngine's
  DevTools port — i.e. the WebDriver/Playwright tier, not this one. The oracle
  harness therefore uses grim for pixels and CDP for the DOM, and does not
  pretend to have an in-app capture path.

## Relation to the VM / `nixosTest` tier

Two tiers, different jobs:

| | this tier (headless sway) | `nixosTest` VM tier |
| --- | --- | --- |
| What boots | host kernel + sway from the Nix store | a full NixOS VM (systemd, udev, kernel) |
| Compositor | sway under `WLR_BACKENDS=headless` | sway/weston inside the VM, graphical test driver |
| Cost | ~4 s per session, no VM | tens of seconds, VM boot |
| Catches | protocol/runtime wiring, our state reporting vs the compositor | packaging, missing runtime deps, session/systemd integration, real DRM |
| Hermetic | mostly: compositor+Qt from Nix, host kernel | yes: everything pinned in the test |

The VM tier is the hermetic acceptance gate; this tier is the fast inner loop
that runs on every change and pins the *semantics* (what we report vs what the
compositor sees). `sway-oracle.sh` is deliberately dependency-light so it also
works in a plain CI container that has Nix but no `/dev/dri` and no systemd.

## Protocol facts and sources

- **xdg-shell has no global position.** The compositor determines a toplevel's
  position; clients set only a *window geometry* offset, and the protocol exposes
  no global coordinates.
  <https://wayland.app/protocols/xdg-shell>
  (spec: `stable/xdg-shell/xdg-shell.xml` in
  <https://gitlab.freedesktop.org/wayland/wayland-protocols>)
- **`wlr-foreign-toplevel-management` gives state, not geometry.** Handles
  expose `title`, `app_id` and `state`; `set_rectangle` is a client→compositor
  hint, with no compositor→client read of a rect.
  <https://wayland.app/protocols/wlr-foreign-toplevel-management-unstable-v1>
- **Qt ignores `Qt::WindowStaysOnTopHint` on Wayland** (no stacking request):
  <https://bugreports.qt.io/browse/QTBUG-73456>
- **wlroots headless backend / env vars** (`WLR_BACKENDS`,
  `WLR_HEADLESS_OUTPUTS`, `WLR_RENDERER`, `WLR_LIBINPUT_NO_DEVICES`):
  <https://gitlab.freedesktop.org/wlroots/wlroots/-/blob/master/docs/env_vars.md>
- **sway configuration and IPC**: `sway(5)` (`xwayland disable`, `output <name>
  scale`, `focus`, `fullscreen`, `move position`) and `swaymsg(1)`
  (`-t get_tree`, `-t get_outputs`):
  <https://github.com/swaywm/sway/blob/master/sway/sway.5.scd> ·
  <https://github.com/swaywm/sway/blob/master/sway/swaymsg.1.scd>
- **grim / wlr-screencopy**: <https://gitlab.freedesktop.org/emersion/grim> ·
  <https://wayland.app/protocols/wlr-screencopy-unstable-v1>
- **NixOS VM tests (`nixosTest`)**: <https://nixos.org/manual/nixos/stable/#sec-nixos-tests>

## Known gaps and honest limits

- **In a GL-less sandbox the compositor has nothing to be an oracle about.** The
  flake checks run in a Nix build sandbox, which has no `/dev/dri`; without EGL Qt
  cannot composite the QtWebEngine widget tree (it flushes the window through QRhi,
  which fails silently), so the client never commits a buffer and `get_tree` has no
  mapped toplevel. The assertions are all still here and run for real wherever EGL
  exists; in that sandbox they are reported as `SKIP` with the quoted evidence and
  the app-side assertions still run. `--require-mapping` turns every skip into a
  failure. Evidence and mechanism: `docs/wayland-smoke-findings.md` §5.
- **No post-load re-probe without a Rust change.** The demo page invokes
  `win_info`/`monitors` once while loading. After the compositor moves or
  fullscreens the window, the runtime is asked nothing further, so the harness
  asserts the *compositor* rect and documents the gap instead of inventing an
  in-app reading. A re-invoke hook (or the T3 test-control channel) would close
  this; it was explicitly out of scope here.
- **`DEMO_OK` is the Wayland pass state, and the harness requires it.** No field
  of the demo's `win_info`/`monitors` results is legitimately unavailable on
  Wayland: positions are answered (above) and the remaining `Err`s — always-on-top
  / -bottom — are not probed by the page. `wayland-smoke.sh` therefore fails on a
  `DEMO_PARTIAL` verdict, which is how a regression to the old position `Err`
  would surface.
- **The record an app gets is its own request, not the compositor's rect.** After
  a compositor-driven move, `outer_position` still reports what the app asked for;
  only the compositor knows where the window really is. Apps that saved geometry
  (`tauri-plugin-window-state`) will therefore restore a position Wayland ignores
  — harmless, and the same thing tao does.
- **Multi-output scale is asserted at the API level.** With `HEADLESS-1`
  at scale 2 the runtime reports `scale=2.0000` and the physical size unchanged
  (`1280x720`), because Qt reports logical geometry (`640x360`) and the runtime
  multiplies by the device-pixel ratio; the harness checks the scale factor and
  the monitor count, not a pixel-level rendering comparison.
