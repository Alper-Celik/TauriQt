<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# cdp-probe — measure QtWebEngine's CDP, and attach Playwright to it

Four dependency-free Node scripts that measure what QtWebEngine's DevTools
server (Qt 6.11.2) actually answers, record what Playwright sends it, and show
the minimal proxy that makes `chromium.connectOverCDP()` work. Measured results
live in `docs/cdp-support-matrix.md`.

| Script | What it does |
|---|---|
| `probe.mjs` | GETs `/json/version` + `/json/list`, opens a WebSocket to the page target, runs a fixed 62-step battery one command at a time, passively records all protocol events (console/exception/network/page/target), writes a full transcript and prints a compact table |
| `mitm.mjs` | Transparent CDP proxy (HTTP + WebSocket) with `stubs` (answer locally) and `rewrites` (map a command onto real ones, optionally on a dedicated page session); logs every message in both directions |
| `playwright-attach.mjs` | `chromium.connectOverCDP()` + a 23-step Playwright smoke (title, evaluate, Tauri `invoke`, console, `pageerror`, click, screenshot, `$eval`, cookies, viewport, `goto`, `newPage`) with per-step outcomes |
| `cdp-lib.mjs` | shared client helpers (global `WebSocket`, structural payload folding) |

Artifacts (already present, regenerable): `last-run.json`,
`last-playwright-attach.json`, `last-playwright-attach-direct.json`,
`last-run-mitm.jsonl`, `stubs.example.json`.

Requirements: Node ≥ 22 (global `WebSocket`, `fetch`; no npm install needed for
the probe/proxy) and the nix dev shell for building/running the app.

## 1. Build and run the app with CDP open

```sh
cd /path/to/TauriQt
LD_LIBRARY_PATH= nix develop -c sh -c 'QMAKE=$(which qmake); cargo build -p tauriqt'
```

Pick a port nobody else is using (the default 9222 collides easily — other
tooling on the same machine will happily attach to *your* app and drive it):

```sh
LD_LIBRARY_PATH= nix develop -c sh -c \
  'QTWEBENGINE_REMOTE_DEBUGGING=9334 QTWEBENGINE_CHROMIUM_FLAGS="--no-sandbox" \
   timeout 300 ./target/debug/tauriqt'
```

`--no-sandbox` is only needed inside the agent firejail; harmless elsewhere.
Leave it running (another terminal / background job). Qt prints
`DevTools listening on ws://127.0.0.1:<port>/devtools/browser/<uuid>`.

## 2. Probe the CDP surface (no dependencies)

```sh
node tools/cdp-probe/probe.mjs --port 9334 --out tools/cdp-probe/last-run.json
```

* prints `/json/version` and `/json/list` verbatim, then a compact table;
* exit `0` when the run completed (individual protocol errors are data, not
  failures), `2` when there is no CDP endpoint or the WebSocket cannot connect;
* per-command timeout **5 s**, `Page.captureScreenshot`/`Page.printToPDF`
  **15 s** (`--timeout-ms` changes the default);
* every command is sequential, so nothing is left running and the app never
  blocks: no nested event loops, no unbounded waits, and the app itself runs
  under `timeout`.

Useful flags: `--host`, `--port`, `--out`, `--timeout-ms`, `--target-id <id|url-substring>`, `--quiet`.

Run it twice to confirm determinism; only the target/session ids, timings and
screenshot bytes differ between runs:

```sh
node tools/cdp-probe/probe.mjs --port 9334 --out /tmp/run1.json
node tools/cdp-probe/probe.mjs --port 9334 --out /tmp/run2.json
```

## 3. Playwright directly (fails, by design) and through the proxy (works)

Install Playwright once, without downloading browsers:

```sh
PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm i --prefix /tmp/cdp-playwright playwright
```

Direct attempt against Qt — reproduces the attach failure named in the matrix
(`Protocol error (Browser.setDownloadBehavior): Browser context management is
not supported.`):

```sh
node tools/cdp-probe/playwright-attach.mjs http://127.0.0.1:9334 \
  --out tools/cdp-probe/last-playwright-attach-direct.json
```

Start the proxy (stubbing exactly the commands Qt does not implement; see
`docs/cdp-support-matrix.md` → "Stub table"):

```sh
node tools/cdp-probe/mitm.mjs \
  --listen 9333 --upstream 127.0.0.1:9334 \
  --log tools/cdp-probe/last-run-mitm.jsonl \
  --stubs tools/cdp-probe/stubs.example.json
```

Then point Playwright at the proxy:

```sh
node tools/cdp-probe/playwright-attach.mjs http://127.0.0.1:9333 \
  --out tools/cdp-probe/last-playwright-attach.json
```

Exit code `0` means attached and every step passed; the one known-unsupported
step (`context.newPage()`, Qt answers `Target.createTarget: Not supported`)
does not fail the run — set `KNOWN_UNSUPPORTED=0` to make it strict. The JSON
records each step with its value or the exact error.

## 4. Proxy details

```
mitm.mjs [--listen N] [--host H] [--upstream HOST:PORT] [--log FILE]
         [--stubs FILE] [--full-log]
```

* HTTP: `/json/version` and `/json/list` are proxied upstream and their
  `webSocketDebuggerUrl`s are rewritten to the proxy address, so
  `http://127.0.0.1:9333` works as an endpoint.
* WS: every upgrade path is piped byte-accurately; both directions are logged
  as JSONL `{"dir":"c2s"|"s2c","at":<ms>,"stubbed"|"rewritten"|"injected":…,"msg":{…}}`.
  Payloads are folded (`shrink`, strings > 200 chars) unless `--full-log`.
* `requests`: `{"<method>": {"result": {…}}}` answers locally, never forwarded.
  Optional `"error": {"code":…,"message":…}`.
* `rewrites`: `{"<method>": {"as":"<real method>", "on":"page"?, "params"?:{}, "split"?: "<param key>", "result"?:{}}}`.
  Without `split` one sub-command is sent; with `split` one per array item
  (e.g. `Storage.setCookies` → N × `Network.setCookie`). `on: "page"` runs the
  sub-commands on a dedicated connection to `/devtools/page/<id>`, because the
  browser endpoint does not expose `Network.*`.
* `injectEvents`: `{"<trigger method>": [<event>, …]}` emits events after that
  command's response, with `$name` substituted from `vars`.
* The stub file is re-read on change (~1 s), so the proxy can stay up while
  iterating; code changes need a restart.

## Notes and limits

* Nothing here writes into the app or the repo outside `tools/cdp-probe/`; runs
  are bounded by `--timeout-ms` per command and by `timeout` on the app process.
* The probe connects as an ordinary CDP client (it does not pause the renderer),
  so it can be run repeatedly against the same app instance; the only lasting
  effects are the page reload from `Page.navigate`, cookies set/cleared via
  `Network.*`, and a `Page.addScriptToEvaluateOnNewDocument` entry that the same
  run removes again.
* `probe.mjs` does not use `Fetch.*` interception for more than
  enable/disable; leaving interception armed without a handler would hang page
  loads.
* QtWebEngine's browser endpoint and page endpoint answer different domains —
  for raw experiments, connect to the page target unless you are testing
  `Browser.*`/`Target.*`.
