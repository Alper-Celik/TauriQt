<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# webdriver-probe — can ChromeDriver attach to QtWebEngine?

Experiment (T1b of `PLAN-WORKERS.md`): point stock **ChromeDriver** at the
QtWebEngine DevTools port of a running TauriQt app and measure how much classic
**W3C WebDriver** works over that attached session. Results are written to
`docs/webdriver-attach-findings.md`.

No npm packages, no Rust changes: the scripts talk to ChromeDriver's HTTP
endpoint with `fetch` and to Qt's DevTools endpoint with plain HTTP GETs.

## Requirements

- a Linux box where the app can open a window (X11 or Wayland) —
  QtWebEngine needs a display session;
- **ChromeDriver**: any copy on `PATH`, or the Nix store copy found
  automatically (the scripts run it with `LD_LIBRARY_PATH=` cleared,
  because the inherited `LD_LIBRARY_PATH` breaks Nix binaries);
- `node` ≥ 18 with `fetch` (used: v24);
- a built `target/debug/tauriqt` — built by `run.sh` when missing.

## Run it

Run these from the repository root (`$PWD` below is that root):

```sh
# 1. bare attach (measured: ChromeDriver refuses — "unrecognized Chrome version")
tools/webdriver-probe/run.sh

# 2. truthful version string (measured: ChromeDriver's own version gate fires)
PROBE_FIXUP_VERSION="Chrome/140.0.7339.207" \
  PROBE_OUT="$PWD/tools/webdriver-probe/probe-out-fixup-truthful" \
  tools/webdriver-probe/run.sh

# 3. version matching the driver (measured: session attaches, 33/39 commands pass)
PROBE_FIXUP_VERSION="Chrome/155.0.8059.39" \
  PROBE_OUT="$PWD/tools/webdriver-probe/probe-out-fixup-spoofed" \
  tools/webdriver-probe/run.sh
```

`PROBE_FIXUP_VERSION` starts `version-fixup.mjs` (HTTP proxy on
`PROBE_FIXUP_PORT`, default 9224) in front of Qt's DevTools port and rewrites the
`Browser` field of `/json/version`; without it ChromeDriver talks to Qt's port
directly. Why that field matters is documented in
`docs/webdriver-attach-findings.md`.

The app is started exactly like `scripts/smoke.sh` does, plus the DevTools port:

```sh
LD_LIBRARY_PATH= nix develop -c sh -c \
  "QTWEBENGINE_REMOTE_DEBUGGING=9223 QTWEBENGINE_CHROMIUM_FLAGS=\"--no-sandbox\" \
   timeout 120 ./target/debug/tauriqt"
```

and ChromeDriver as:

```sh
chromedriver --port=4444 --verbose --log-path=tools/webdriver-probe/probe-out/chromedriver-verbose.log
```

Run the probe against an app you started yourself (useful when iterating):

```sh
node tools/webdriver-probe/probe.mjs --cdp=127.0.0.1:9223 --port=4444
```

Options: `--cdp=HOST:PORT`, `--port=CHROMEDRIVER_PORT`, `--out=DIR`,
`--chromedriver=/path/to/chromedriver`, `--fixup=CHROME_VERSION`,
`--fixup-port=PORT`, `--keep-driver`.

## What the probe does

1. `GET /json/version` and `/json/list` on the DevTools port (printed verbatim).
2. `chromedriver --version`; compares its major version with the target's
   reported Chromium (`Chrome/140.0.0.0` for QtWebEngine 6.11).
3. `POST /session` with
   `{"capabilities":{"alwaysMatch":{"browserName":"chrome","goog:chromeOptions":{"debuggerAddress":"127.0.0.1:9223"}}}}`
   — the full response is stored. With `--fixup` the debugger address points at
   `version-fixup.mjs` instead of Qt's port.
4. With a session, drives the classic command battery: `url` (get/set to
   `tauri://localhost`), `title`, `source`, `execute/sync` (`document.title`,
   `invoke('greet')`), `execute/async` (`invoke('greet')`), element find single
   and plural, `text`/`attribute`/`property value`/`displayed`/`enabled`/
   `clear`/`click`/`send-keys`, an executed click and typed text whose effects
   the probe reads back (including `event.isTrusted`), `screenshot`, `cookie`
   get/set, `window/rect` get/set, `timeouts`, `window/handles`,
   `DELETE /session/{id}`. Every request/response is recorded, failures
   included.
5. Scans the ChromeDriver `--verbose` log and separates **CDP-level rejections**
   (method + code + verbatim message, e.g. `'Browser.getWindowForTarget' wasn't
   found`) from **refusals ChromeDriver made on its own** (no CDP command sent,
   e.g. `invalid cookie domain`), plus a census of every CDP method it sent.

## Artifacts (`probe-out/`, override with `--out`)

| File | Content |
|---|---|
| `results.json` | every step: request, HTTP status, body, error string, screenshot metadata, CDP command census, CDP rejections, driver-side refusals |
| `screenshot.png` | base64-decoded WebDriver screenshot (only when the command succeeded) |
| `chromedriver-verbose.log` | full ChromeDriver trace, including every CDP command it sent |
| `app.log` | copy of the app's output for that run (carries `DEMO_OK` on a full demo pass) |

`run.sh` prints the app's `[tauri-runtime-qt]` trace lines after the probe. It
starts the app in its own process group (`setsid`), refuses to run when the CDP
port is held by a foreign process, and on exit tears down the whole
`nix develop → timeout → tauriqt` chain plus ChromeDriver — the app itself also
carries `timeout 120` (`PROBE_APP_TIMEOUT`).
