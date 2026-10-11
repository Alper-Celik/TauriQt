<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT -->

# CDP support matrix — QtWebEngine (TauriQt runtime)

Measured, not guessed: everything below was produced by the probe in
`tools/cdp-probe/` against the demo app (`tauriqt`, page `tauri://localhost/`)
running on this runtime with `QTWEBENGINE_REMOTE_DEBUGGING` open. Raw data:

| File | Content |
|---|---|
| `tools/cdp-probe/last-run.json` | full probe transcript: 62 commands, 96 passive events (30 methods) |
| `tools/cdp-probe/last-playwright-attach.json` | Playwright 1.64.0 over the proxy: 23 steps, 22 ok, 1 known-unsupported (`context.newPage()`) |
| `tools/cdp-probe/last-playwright-attach-direct.json` | Playwright against Qt directly: the exact attach failure |
| `tools/cdp-probe/last-run-mitm.jsonl` | every CDP message Playwright sent/received during the recorded attach (folded payloads) |

## Environment

| | |
|---|---|
| Engine | QtWebEngine 6.11.2, Chromium 140.0.0.0, V8 14.0.365.10 |
| App | `cargo build -p tauriqt`, `QTWEBENGINE_REMOTE_DEBUGGING=<port>` |
| Page target | `tauri://localhost/`, target id `62333976D22645C15DFC92F6D6CFA479` (== frame id) |
| Probe client | Node v24.21.0, global `WebSocket`, **no npm dependencies** |
| Playwright | 1.64.0 (`playwright-core` 1.64.0), browsers not downloaded |
| Command timeouts | **5 000 ms** default; **15 000 ms** for `Page.captureScreenshot` / `Page.printToPDF` |
| Timeouts observed | **none** — all 9 failures are protocol errors (`-32601` / `-32000` / `-32602`), not timeouts |
| Statuses | 49 `result`, 9 `error`, 4 pauses |

Reproduce with `tools/cdp-probe/README.md`.

## Endpoints

`GET /json/version` (HTTP 200):

```json
{
  "Browser": "tauri-runtime-qt/",
  "Protocol-Version": "1.3",
  "User-Agent": "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36 (KHTML, like Gecko) QtWebEngine/6.11.2 Chrome/140.0.0.0 Safari/537.36",
  "V8-Version": "14.0.365.10",
  "WebKit-Version": "537.36 (@0000000000000000000000000000000000000000)",
  "webSocketDebuggerUrl": "ws://127.0.0.1:9334/devtools/browser/71d71e4d-a467-4519-bd5b-1856a8f2fc03"
}
```

`GET /json/list` (HTTP 200) returns exactly one `page` target with
`id` == `Page.getFrameTree` frame id, `url` `tauri://localhost/`, `title`
`DEMO_OK`, and its own `webSocketDebuggerUrl`; the id is stable across
`Page.navigate`. Qt prints `Added {--remote-allow-origins=*} to command-line
arguments …` to the app log, so no `Origin`/allow-list handling is needed
(Node's WebSocket sends none anyway).

**Two session kinds, different domain coverage.** The browser endpoint
(`/devtools/browser/<uuid>`) is not a superset of a page session:

| Command on the **browser** endpoint (root session) | Result |
|---|---|
| `Network.enable` | `-32601 'Network.enable' wasn't found` |
| `Network.getAllCookies` | `-32601 'Network.getAllCookies' wasn't found` |
| `Target.attachToTarget {targetId, flatten:true}` | ok → `{sessionId}` |
| `Network.getAllCookies` **session-scoped** (via that sessionId) | ok → `{"cookies":[]}` |
| `Runtime.evaluate` session-scoped | ok → `{"result":{"type":"string","value":"DEMO_OK"}}` |

So page/runtime/network domains live on the page session; anything that must
run against the page has to attach (or connect to the page endpoint).

## Command matrix (measured)

Battery as executed by `probe.mjs`; `<pause>` rows are settle windows for the
passive event log. Exact error strings are Qt's, verbatim.

| # | Command | Status | Detail |
|---|---|---|---|
| 1 | `Browser.getVersion` | ok | `result` |
| 2 | `Browser.getBrowserCommandLine` | ERROR | `-32000 Command line not returned because --enable-automation not set.` |
| 3 | `Browser.getWindowForTarget` | ERROR | `-32601 'Browser.getWindowForTarget' wasn't found` |
| 4 | `Browser.getWindowBounds` | ERROR | `-32601 'Browser.getWindowBounds' wasn't found` |
| 5 | `Browser.setDownloadBehavior` | ERROR | `-32000 Browser context management is not supported.` |
| 6 | `Target.getTargets` | ok | `result` |
| 7 | `Target.getBrowserContexts` | ERROR | `-32000 Not allowed` |
| 8 | `Target.setDiscoverTargets` | ok | `result` |
| 9 | `Target.getTargetInfo` | ok | `result` |
| 10 | `Target.setAutoAttach` | ok | `result` |
| 11 | `Target.setAutoAttach` (Playwright shape: `waitForDebuggerOnStart` + `filter`) | ok | `result` |
| 12 | `Target.attachToTarget {flatten:true}` | ok | `result` (returns `sessionId`) |
| 13 | `Runtime.evaluate` via flat `sessionId` | ok | `result` |
| 14 | `Page.getFrameTree` via flat `sessionId` | ok | `result` |
| 15 | `Target.detachFromTarget` | ok | `result` |
| 16 | `Page.enable` | ok | `result` |
| 17 | `Runtime.enable` | ok | `result` |
| 18 | `Log.enable` | ok | `result` |
| 19 | `<pause>` (settle: passive event window after enables) | pause | — |
| 20 | `Page.getFrameTree` | ok | `result` |
| 21 | `Page.setLifecycleEventsEnabled` | ok | `result` |
| 22 | `Page.addScriptToEvaluateOnNewDocument` | ok | `result` (`identifier: "1"`) |
| 23 | `Page.removeScriptToEvaluateOnNewDocument` | ok | `result` |
| 24 | `Page.createIsolatedWorld` | ok | `result` (`executionContextId`) |
| 25 | `DOM.getDocument {depth:1}` | ok | `result` |
| 26 | `DOM.querySelector {nodeId, '#summary'}` | ok | `result` (`nodeId: 10`) |
| 27 | `CSS.enable` | ok | `result` |
| 28 | `Runtime.evaluate '1+1'` | ok | `result` (`number 2`) |
| 29 | `Runtime.evaluate 'document.title'` | ok | `result` (`"DEMO_OK"`) |
| 30 | `Runtime.evaluate` console probe | ok | `result`; both `Runtime.consoleAPICalled` events arrived |
| 31 | `Runtime.evaluate` exception probe | ok | `result`; `Runtime.exceptionThrown` arrived with full details |
| 32 | `Runtime.evaluate` `awaitPromise` → `invoke('greet',{name:'cdp'})` | ok | `result` `"Hello, cdp! — from linux"` |
| 33 | `Runtime.evaluate` `awaitPromise` → `invoke('report',{text})` | ok | `result`; app log shows `[tauriqt-demo] cdp-probe: evaluate reached Rust` |
| 34 | `<pause>` (settle: console/exception events) | pause | — |
| 35 | `Network.enable` | ok | `result` |
| 36 | `Network.getAllCookies` | ok | `result` `{"cookies":[]}` |
| 37 | `Network.getCookies {urls:['tauri://localhost/']}` | ok | `result` `{"cookies":[]}` |
| 38 | `Network.setCookie {url:'tauri://localhost/'}` | ERROR | `-32602 URL must have scheme http or https` |
| 39 | `Network.setCookie {url:'http://localhost/'}` | ok | `result` `{"success":true}` |
| 40 | `Network.getAllCookies` (after setCookie) | ok | `result` 1 cookie, full shape below |
| 41 | `Storage.getCookies` | ERROR | `-32000 Browser context management is not supported.` |
| 42 | `Network.deleteCookies {url:'tauri://localhost/'}` | ERROR | `-32602 An http or https url URL must be specified` |
| 43 | `Network.deleteCookies {url:'http://localhost/'}` | ok | `result` |
| 44 | `Network.clearBrowserCookies` | ok | `result` |
| 45 | `Runtime.evaluate` (arm input observers + contenteditable) | ok | `result` (`activeElement: cdp-probe-input`) |
| 46 | `Input.dispatchMouseEvent mousePressed` | ok | `result` |
| 47 | `Input.dispatchMouseEvent mouseReleased` | ok | `result` |
| 48 | `Input.insertText` | ok | `result` |
| 49 | `Input.dispatchKeyEvent keyDown` | ok | `result` |
| 50 | `Input.dispatchKeyEvent keyUp` | ok | `result` |
| 51 | `Runtime.evaluate` (read back input effects) | ok | `result` — see "Input" |
| 52 | `Emulation.setDeviceMetricsOverride` | ok | `result` |
| 53 | `Emulation.clearDeviceMetricsOverride` | ok | `result` |
| 54 | `Page.captureScreenshot {format:'png'}` | ok | `result` 210 828 B PNG, IHDR 1277×1562 |
| 55 | `Page.printToPDF` | ERROR | `-32601 'Page.printToPDF' wasn't found` |
| 56 | `Security.enable` | ok | `result` |
| 57 | `Fetch.enable {}` | ok | `result` |
| 58 | `Fetch.disable` | ok | `result` (cleanup; never left armed) |
| 59 | `Page.navigate {url:'tauri://localhost/'}` | ok | `result` `{frameId, loaderId, isDownload:false}` |
| 60 | `<pause>` (settle: navigation events) | pause | — |
| 61 | `Runtime.evaluate 'document.title'` after reload | ok | `result` (page reloaded; demo re-runs) |
| 62 | `<pause>` (final drain) | pause | — |

Selected result payloads (verbatim shapes):

```json
// 1 Browser.getVersion
{"protocolVersion":"1.3","product":"tauri-runtime-qt/","revision":"@0000000000000000000000000000000000000000",
 "userAgent":"…QtWebEngine/6.11.2 Chrome/140.0.0.0…","jsVersion":"14.0.365.10"}

// 9/12 Target.getTargetInfo + Target.attachToTarget
{"targetInfo":{"targetId":"62333976D22645C15DFC92F6D6CFA479","type":"page","title":"DEMO_OK",
 "url":"tauri://localhost/","attached":true,"canAccessOpener":false,
 "browserContextId":"5D5D542B676D651C73859163D1830E67"}}
{"sessionId":"BA7DD39F777B5B32A083102BADC9D38E"}

// 40 Network.getAllCookies (after Network.setCookie on http://localhost/)
{"cookies":[{"name":"cdp_probe_http","value":"1","domain":"localhost","path":"/","expires":-1,"size":15,
 "httpOnly":false,"secure":false,"session":true,"priority":"Medium","sameParty":false,
 "sourceScheme":"NonSecure","sourcePort":80}]}
```

## Events (passive, 96 total, 30 methods)

Everything below arrived without any `Fetch`/`Page` trickery, just from
`Page.enable` + `Runtime.enable` + `Log.enable` + `Network.enable` and normal
page activity (one reload at step 59):

| Count | Event | Count | Event |
|---|---|---|---|
| 18 | `Runtime.executionContextCreated` | 2 | `DOM.setChildNodes` |
| 13 | `Page.lifecycleEvent` | 2 | `CSS.styleSheetAdded` |
| 10 | `CSS.mediaQueryResultChanged` | 2 | `Runtime.consoleAPICalled` |
| 6 | `Network.requestWillBeSent` | 2 | `DOM.childNodeCountUpdated` |
| 6 | `Network.responseReceived` | 2 | `DOM.documentUpdated` |
| 6 | `Network.dataReceived` | 1 | `CSS.styleSheetRemoved` |
| 6 | `Network.loadingFinished` | 1 | `DOM.childNodeInserted` |
| 4 | `Page.frameResized` | 1 | `DOM.childNodeRemoved` |
| 1 | `DOM.scrollableFlagUpdated` | 1 | `Page.frameStartedNavigating` |
| 1 | `Page.frameStartedLoading` | 1 | `Page.frameStoppedLoading` |
| 1 | `Page.frameNavigated` | 1 | `Page.domContentEventFired` |
| 1 | `Page.loadEventFired` | 1 | `Network.policyUpdated` |
| 1 | `Runtime.exceptionThrown` | 1 | `Runtime.executionContextsCleared` |
| 1 | `Target.attachedToTarget` | 1 | `Target.detachedFromTarget` |
| 1 | `Target.targetCreated` | 1 | `Target.targetInfoChanged` |

(Counts vary a little run to run — an earlier run recorded 110 events with the
same 30 methods.)

Console/exception visibility is real and complete (`Runtime.consoleAPICalled`
carries `type` + `args` + `stackTrace`; `Runtime.exceptionThrown` carries
`exceptionDetails.exception.description` with the stack). `Log.enable` produced
no `Log.entryAdded` during this run (nothing was logged to the browser log);
`js:` lines in the app log correspond to the console probe.

**Caveat:** the `Target.*` lifecycle events cover the single page target; there
are no other targets to observe (see "no new targets" below).

## Input: synthetic CDP input is trusted and takes effect

The probe armed document-level listeners, focused a scratch `contenteditable`,
then dispatched a click at (40,60), `Input.insertText 'cdp-probe-text'`, and a
`keyDown/keyUp` for `a`, then read the effects back from inside the page:

```json
{"clicks":[{"x":40,"y":60,"isTrusted":true}],
 "keys":[{"key":"a","isTrusted":true}],
 "inputs":[{"inputType":"insertText","data":"cdp-probe-text","isTrusted":true},
           {"inputType":"insertText","data":"a","isTrusted":true}],
 "activeElement":"cdp-probe-input","boxText":"cdp-probe-texta"}
```

So `Input.dispatchMouseEvent` / `dispatchKeyEvent` / `insertText` are routed
through the real input pipeline: events are `isTrusted: true`, hit-testing and
text insertion actually happen (unlike DOM-dispatched synthetic events).

## The attach problem: Playwright cannot connect to Qt directly

`chromium.connectOverCDP()` against Qt fails identically for all three endpoint
forms (`http://127.0.0.1:<port>`, `ws://…/devtools/browser/<id>`,
`ws://…/devtools/page/<id>`):

```
browserType.connectOverCDP: Protocol error (Browser.setDownloadBehavior):
Browser context management is not supported.
```

This is the raw `Browser.setDownloadBehavior` rejection from the matrix row 5,
surfaced by Playwright's own error message — measured, not inferred
(`last-playwright-attach-direct.json`). In `playwright-core@1.64.0`
(`lib/coreBundle.js`, line 39994) `CRBrowserContext.initialize()` sends that
command unconditionally for non-Clank browsers, during
`connectOverCDP`'s default-context setup, before any page exists. Qt answers
`-32000 Browser context management is not supported.` and Playwright aborts.

Two further Playwright-side facts (from the same bundle, cited so the proxy
author knows what to expect, not measured errors):

* `Browser.getVersion` parsing (line 39765) takes the substring after `/` from
  `product` — Qt reports `tauri-runtime-qt/`, so `browser.version()` is `""` and
  `_majorVersion` is `NaN`. Playwright tolerates it (version-gated code paths
  then take the "new" branch); ChromeDriver does not and requires a
  `Chrome/<major>.<minor>.<build>.<patch>` product (T1b's finding).
* `Browser.getWindowForTarget` (line 38966) is wrapped in `try {} catch {}` and
  `Browser.getWindowBounds`/`setWindowBounds` (lines 39418/39424) are only
  reached when a window id was obtained — which is why the missing window
  commands do **not** block attach or `page.setViewportSize()`
  (which worked over `Emulation.setDeviceMetricsOverride`).

### What the proxy had to synthesise (and what it did not)

`tools/cdp-probe/mitm.mjs` is a transparent CDP proxy; with exactly this stub
set, Playwright 1.64.0 attaches and drives the app (22/23 steps ok; only
`context.newPage()` fails, see below). The complete client→upstream method
sequence is recorded in `last-run-mitm.jsonl`; it is
`Browser.getVersion`, `Target.setAutoAttach`, `Browser.setDownloadBehavior`
[stubbed], `Target.getTargetInfo`, then per page session:
`Page.enable`, `Page.getFrameTree`, `Log.enable`,
`Page.setLifecycleEventsEnabled`, `Runtime.enable`,
`Page.addScriptToEvaluateOnNewDocument`, `Network.enable`,
`Target.setAutoAttach`, `Emulation.setFocusEmulationEnabled`,
`Emulation.setEmulatedMedia`, `Inspector.enable`,
`Runtime.runIfWaitingForDebugger`, `Page.createIsolatedWorld`,
then `Runtime.evaluate`/`Runtime.callFunctionOn`/`DOM.*`/`Input.*`/
`Page.captureScreenshot`/`Emulation.setDeviceMetricsOverride`/`Page.navigate` as
the test drives the page. **Everything in that list except
`Browser.setDownloadBehavior` (and the `Storage.*` cookie calls below) is
answered by Qt itself.**

### Stub table (response shapes from the real Chromium CDP schema)

Shapes are from `playwright-core@1.64.0`'s bundled `types/protocol.d.ts`
(generated from Chromium's CDP schema), line numbers cited so they can be
re-checked.

| # | Command | Needed for | Synthesised response | Schema source |
|---|---|---|---|---|
| 1 | `Browser.setDownloadBehavior` | **attach** (`CRBrowserContext.initialize`, coreBundle.js:39994) | `{}` — the command's return value is a void `setDownloadBehaviorReturnValue` | protocol.d.ts:2189 (`setDownloadBehaviorReturnValue`) |
| 2 | `Storage.getCookies` | `context.cookies()` | **rewrite** to `Network.getAllCookies` **on a page session** → `{"cookies":[Cookie…]}` | protocol.d.ts:13051 (`getCookiesReturnValue`); `Network.getAllCookies` returns the identical `{"cookies": Cookie[]}` |
| 3 | `Storage.setCookies` | `context.addCookies()` | **rewrite** to one `Network.setCookie` per cookie (drop `browserContextId`) → `{}` | protocol.d.ts:13248 (`setCookiesReturnValue` void); `setCookie` returns `{"success":boolean}` |
| 4 | `Storage.clearCookies` | `context.clearCookies()` | **rewrite** to `Network.clearBrowserCookies` (page session) → `{}` | protocol.d.ts:18243 (`clearCookiesReturnValue` void) |
| 5 | `Browser.getWindowForTarget` | window-aware viewport handling; **optional** (Playwright catches the failure) | `{"windowId":1,"bounds":{"left":0,"top":0,"width":1280,"height":800,"windowState":"normal"}}` | protocol.d.ts:2346 (`getWindowForTargetReturnValue`), `Bounds` at ~1960: `left?/top?/width?/height?/windowState?` |
| 6 | `Browser.getWindowBounds` | `page` window state; **optional** (only reached with a windowId) | `{"bounds":{"left":0,"top":0,"width":1280,"height":800,"windowState":"normal"}}` | protocol.d.ts:2330 (`getWindowBoundsReturnValue`) |
| 7 | `Browser.setWindowBounds` | `page.setViewportSize` in window mode; **optional** (works via `Emulation.*` today) | `{}` | protocol.d.ts:2371 (`setWindowBoundsReturnValue` void) |
| 8 | `Target.getBrowserContexts` | clients that enumerate contexts (`browser.contexts()` is served from the attach path, which worked without it) | `{"browserContextIds":[]}` | protocol.d.ts:18935 (`getBrowserContextsReturnValue`) |
| 9 | `Browser.getVersion` | version reporting; **optional** | pass-through is fine for Playwright; if a client needs a real major version, report `"product": "Chrome/140.0.0.0"` (Qt's engine) instead of `tauri-runtime-qt/` | protocol.d.ts:2232 (`getVersionReturnValue`) |
| 10 | `Target.createTarget` | `context.newPage()` | **cannot be synthesised honestly** — “Not supported” in Qt, and there is no second webview to attach to. Return an error. | protocol.d.ts:19011 (`createTargetReturnValue`) |

Notes: the proxy's `requests` stubs answer locally and never reach Qt;
`rewrites` map one client command onto one or more real commands and must run
on a **page session** because the browser endpoint does not expose `Network.*`
(measured above). `Storage.setCookies` for a `tauri://` URL cannot be honoured
even via the rewrite — Chromium rejects cookie commands for non-http(s) schemes
(matrix rows 38/42).

### Playwright-over-proxy results (Playwright 1.64.0, `last-playwright-attach.json`)

| Playwright call | Result |
|---|---|
| `connectOverCDP('http://127.0.0.1:9333')` | ok (via `Browser.setDownloadBehavior` stub) |
| `browser.version()` | `""` (Qt product string; see above) |
| `browser.contexts().length` | 1 |
| `page.title()` / `page.url()` | `DEMO_OK` / `tauri://localhost/` |
| `page.evaluate('1+1')` | 2 |
| `page.evaluate(invoke('greet'))` | `"Hello, playwright! — from linux"` |
| `page.evaluate(invoke('report'))` | reaches Rust (`[tauriqt-demo]` line in the app log) |
| `page.on('console')` / `page.on('pageerror')` | both fire |
| `page.click('#summary')` | ok |
| `page.screenshot()` | 210 826 B PNG |
| `page.$eval('#summary')` | `DEMO_OK — every probed command succeeded` |
| `context.cookies()` → `addCookies` → `cookies()` → `clearCookies()` → `cookies()` | `[]` → added → `["pw_probe"]` → cleared → `[]` (via rewrites) |
| `page.setViewportSize({800,600})` / `page.viewportSize()` | ok (via `Emulation`) |
| `page.goto('tauri://localhost/')` | ok, `status=200`, 16 `request` events |
| `context.newPage()` | **FAILS**: `Protocol error (Target.createTarget): Not supported` |
| `browser.close()` | ok (connection closes; app keeps running) |

## Implications

What this buys for automated testing on this runtime:

* **JS-level testing is fully available.** `Runtime.evaluate` (incl.
  `awaitPromise`, `returnByValue`), `Runtime.callFunctionOn`,
  `Page.addScriptToEvaluateOnNewDocument`, `Page.createIsolatedWorld`,
  `DOM.getDocument`/`querySelector`/`resolveNode`/`describeNode` and console /
  exception capture all work. A Tauri `invoke()` resolves end-to-end inside
  `Runtime.evaluate` (`awaitPromise`) and the Rust side sees it — so page-level
  integration tests (assert UI state, call commands, assert results) are
  possible today, as is using the runtime's own event/title trace as a channel.
* **Playwright needs the proxy, but the proxy is small.** One stub
  (`Browser.setDownloadBehavior → {}`) is enough to attach; three cookie
  rewrites make `context.cookies()`/`addCookies`/`clearCookies` real. Window
  bounds stubs are optional (Playwright degrades to `Emulation.*`, and
  `page.setViewportSize` verified working).
* **Cookies: CDP is the authoritative enumeration path.**
  `Network.getAllCookies` / `Network.getCookies` return the live cookie store
  (with `domain/path/expires/httpOnly/secure/session/sameSite/priority/
  sourceScheme/sourcePort`), and `Network.setCookie`/`deleteCookies`/
  `clearBrowserCookies` write. Caveats: commands only accept `http(s)` URLs —
  a `tauri://` URL is rejected with `-32602` — and `Storage.*` (delegating to
  the browser context) is unimplemented. This is the route the W4 note "kept in
  reserve" refers to: a page-session `Network.getAllCookies` gives a real,
  completion-signalled answer for cookies the app's own Qt mirror may lag on.
* **Screenshots work; PDF does not.** `Page.captureScreenshot` returns a PNG
  with real pixels (210 828 B, IHDR 1277×1562 px in this run; the two recorded
  runs produced 199 845 B and 210 828 B for the same window, content-dependent),
  which is enough for visual assertions; `Page.printToPDF` is `-32601`. Qt-side
  `QWidget::grab` (T3) is only needed for whole-window or native-chrome captures.
* **Real input works.** `Input.*` events are `isTrusted: true` and have real
  effects, so CDP-side clicking/typing is a legitimate substitute for
  OS-level input for web content (the T3 test-control channel is still the way
  to drive the native window/menu).
* **Window operations are browser-level and missing.** `Browser.getWindowForTarget`,
  `getWindowBounds`, `setWindowBounds`, `getBrowserCommandLine` and
  browser-context management are all absent; window geometry, focus, fullscreen
  and app-level control must come from the app (T2b/T3), not from CDP.
* **One page, no new targets.** `Target.getTargets` lists one page; `Target.createTarget`
  answers `Not supported`, so `context.newPage()`, new browser contexts, popups
  and multi-window scenarios cannot be tested over CDP. `Target.attachToTarget`
  with `flatten:true` does work and Qt emits `Target.attachedToTarget`, so
  single-page session routing is genuine.
* **Browser-session vs page-session matters.** A proxy/client must attach or
  connect to the page endpoint for `Network.*`/`Runtime.*`; the browser
  endpoint only answers `Browser.*`/`Target.*`. Proxies must therefore route
  page-scoped commands through a page session (the proxy does this with a
  second WS to `/devtools/page/<id>`).

## Not measured / limitations of this matrix

* **Fetch interception was only enabled and disabled** (`Fetch.enable {}` →
  ok, `Fetch.disable` → ok); whether Qt honours real interception patterns and
  `Fetch.requestPaused`/`continueRequest` was not exercised (deliberately: the
  probe must not leave the page paused).
* `Log.enable` is accepted; no `Log.entryAdded` was produced by the page in
  these runs (console output arrives via `Runtime.consoleAPICalled`).
* Not probed: dialogs (`Page.javascriptDialogOpening` — an `alert()` could wedge
  a renderer whose dialog handling is not wired yet), downloads, screencast,
  tracing, `Browser.close`, `Input.dispatchDragEvent`, mobile emulation beyond
  `setDeviceMetricsOverride`, and `Target.activateTarget` (T1b measured the
  latter as working).
* Only the `tauri://localhost/` page on QtWebEngine 6.11.2 was measured; Qt
  dev-branch behaviour may differ.

## Second data point: a third-party app (Readest, 2026-10-11)

The same battery was later run against a large application this repository did not write —
Readest on `tauri-runtime-qt` (`integrations/readest/check.mjs` opens it; the run is recorded
in `integrations/readest/README.md`). Result: **62 commands → 55 results, 7 errors, 0
timeouts**, 1175 events recorded, and the seven failures are exactly the Qt gaps listed in the
matrix above (`Browser.getWindowForTarget`, `Browser.getWindowBounds`, `Page.printToPDF`, and
the browser-context family: `Browser.setDownloadBehavior`, `Target.getBrowserContexts`,
`Storage.getCookies`, `Browser.getBrowserCommandLine`). So the matrix is a property of the
engine, not of the demo page — and the same caveat applies: anything that needs a *new*
target (`Target.createTarget`, Playwright's `context.newPage()`) has no path here, which is
also what blocks WebDriver-driven suites
([webdriver-attach-findings.md](webdriver-attach-findings.md)).
