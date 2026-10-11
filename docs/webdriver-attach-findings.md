<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT -->

# ChromeDriver attach against QtWebEngine — measured findings (T1b)

Date: 2026-10-11 · Machine: aarch64 Linux (Wayland session, X11-capable), firejail sandbox
App: `tauriqt` (`target/debug/tauriqt`, built from this worktree) · Probe: `tools/webdriver-probe/`

Question from `PLAN-WORKERS.md` (T1b): can stock **ChromeDriver** be pointed at our app's
QtWebEngine DevTools port, and if so, how much of classic **W3C WebDriver** works?

## Verdict

> **Attach works — with one required fix-up, and two command families missing.**
>
> - Bare attach (no fix-up): **impossible**. ChromeDriver aborts session creation before sending
>   a single CDP command, because QtWebEngine reports `"Browser": "tauri-runtime-qt/"` in
>   `/json/version` and ChromeDriver cannot parse it.
> - With `"Browser"` rewritten to a Chrome-shaped, **major-matching** version
>   (`Chrome/155.0.8059.39` for the ChromeDriver 155 available here): session creation succeeds and
>   **33 of 39 probed endpoints pass**, including `execute/sync` returning `document.title`, a real
>   Tauri `invoke('greet')` round-trip (sync *and* async), element find/text/click/value, and a
>   genuine PNG screenshot.
> - Remaining failures: **`window/rect` (get+set)** — Qt's CDP does not implement
>   `Browser.getWindowForTarget` — and **cookie set** on the `tauri://` origin (ChromeDriver refuses
>   it itself). Two further failures are only the legacy JSON-Wire-Protocol
>   `GET /element/{id}/value` endpoint that ChromeDriver always refuses in W3C mode; they are not
>   Qt limitations.
> - Recommendation: **chromedriver + fix-ups** for the W3C core, with the T2 shim narrowed to the
>   two missing families (window rect, cookies) served from the app's own state (T3 test-control
>   channel). Details in [Recommendation](#recommendation-for-the-test-stack).

## Environment (measured)

| Component | Value |
|---|---|
| QtWebEngine | `6.11.2`, Chromium `140.0.0.0` (`User-Agent … QtWebEngine/6.11.2 Chrome/140.0.0.0 Safari/537.36`), V8 `14.0.365.10`, Blink `537.36` |
| DevTools endpoint | `GET http://127.0.0.1:9223/json/version`, `/json/list` (HTTP/1.1 200) |
| ChromeDriver | `ChromeDriver 155.0.8059.39 (3ff7ac5a9224be9156d7f8703a06e22890aafd34-refs/tags/155.0.8059.39@{#0})`, `nixpkgs` build, `--port=4444` |
| Driver/browser gap | driver expects Chrome **155**, browser is Chromium **140** (a 25-major gap; no version-matched driver is installed here) |
| node | `v24.21.0` (probe scripts use `fetch`, zero npm dependencies) |
| Page under test | `tauri://localhost/`, title `TauriQt — runtime demo` (after the demo finishes: `DEMO_OK`); one `page` target in `/json/list` |

`/json/version` as Qt answers it (verbatim, `<guid>` elided):

```json
{
   "Browser": "tauri-runtime-qt/",
   "Protocol-Version": "1.3",
   "User-Agent": "Mozilla/5.0 (X11; Linux aarch64) AppleWebKit/537.36 (KHTML, like Gecko) QtWebEngine/6.11.2 Chrome/140.0.0.0 Safari/537.36",
   "V8-Version": "14.0.365.10",
   "WebKit-Version": "537.36 (@0000000000000000000000000000000000000000)",
   "webSocketDebuggerUrl": "ws://127.0.0.1:9223/devtools/browser/<guid>"
}
```

`/json/list` (verbatim shape):

```json
[ {
   "description": "",
   "devtoolsFrontendUrl": "/devtools/inspector.html?ws=127.0.0.1:9223/devtools/page/D79CFF6C53A5E6B313FFB0C524551648",
   "id": "D79CFF6C53A5E6B313FFB0C524551648",
   "title": "TauriQt — runtime demo",
   "type": "page",
   "url": "tauri://localhost/",
   "webSocketDebuggerUrl": "ws://127.0.0.1:9223/devtools/page/D79CFF6C53A5E6B313FFB0C524551648"
} ]
```

## Session-creation experiments (all via `POST /session`)

Capabilities sent (the classic attach recipe):

```json
{"capabilities":{"alwaysMatch":{"browserName":"chrome","goog:chromeOptions":{"debuggerAddress":"127.0.0.1:9223"}}}}
```

| # | `/json/version` `Browser` seen by ChromeDriver | HTTP | Result |
|---|---|---|---|
| 1 | `tauri-runtime-qt/` (Qt's raw value) | 500 | `session not created: cannot connect to chrome at 127.0.0.1:9223` / `from unknown error: unrecognized Chrome version: tauri-runtime-qt/` |
| 2 | `Chrome/140.0.0.0` (Qt's UA version, 4 components but build `0`) | 500 | `from unknown error: unrecognized Chrome version: Chrome/140.0.0.0` |
| 3 | `Chrome/140.0.7339.207` (realistic 140 build, **truthful** to the engine) | 500 | `from session not created: This version of ChromeDriver only supports Chrome version 155` / `Current browser version is 140.0.7339.207` |
| 4 | `Chrome/155.0.8059.39` (matches the driver) | **200** | session created, full command battery driven |

Rows 1/2/3 are recorded in `tools/webdriver-probe/probe-out/`,
`…/probe-out-fixup-truthful/`; row 4 in `…/probe-out-fixup-spoofed/`. Row 1 is with ChromeDriver
talking to Qt directly; rows 2–4 additionally show that the failure is **not** about the port or
the WebSocket, only about that one JSON field.

Verbatim error bodies (rows 1–3), with the top stack frames ChromeDriver returned — they name the
exact code path, and prove **no CDP command was ever sent** (`cdpCommands: {}` in those runs):

```
session not created: cannot connect to chrome at 127.0.0.1:9223
from unknown error: unrecognized Chrome version: tauri-runtime-qt/
  … Status::Status() | BrowserInfo::ParseBrowserString() | BrowserInfo::ParseBrowserInfo()
    | DevToolsHttpClient::Init() | WaitForDevToolsAndCheckVersion() | LaunchChrome() | InitSessionHelper()
```

```
session not created: cannot connect to chrome at 127.0.0.1:9224
from unknown error: unrecognized Chrome version: Chrome/140.0.0.0
  … Status::Status() | BrowserInfo::ParseBrowserString() | BrowserInfo::ParseBrowserInfo()
    | DevToolsHttpClient::Init() | WaitForDevToolsAndCheckVersion() | LaunchChrome()
```

```
session not created: cannot connect to chrome at 127.0.0.1:9224
from session not created: This version of ChromeDriver only supports Chrome version 155
Current browser version is 140.0.7339.207
  … Status::Status() | CheckVersion() | WaitForDevToolsAndCheckVersion() | LaunchChrome() | InitSessionHelper()
```

## Why the fix-up is needed (source level)

1. QtWebEngine builds the field from the **application name/version**:

   ```cpp
   // qtwebengine 6.11.2, src/core/content_browser_client_qt.cpp
   std::string ContentBrowserClientQt::GetProduct()
   {
       QString productName(qApp->applicationName() % u'/' % qApp->applicationVersion());
       return productName.toStdString();
   }
   ```

   and Chromium's DevTools HTTP handler publishes it as `Browser`
   (`content/browser/devtools/devtools_http_handler.cc`: `version.Set("Browser", GetContentClient()->browser()->GetProduct())`).

2. TauriQt hands Qt a fake `argv0` (`tauri-runtime-qt/src/qt_app.cpp`: `static char argv0[] = "tauri-runtime-qt";`)
   and never calls `setApplicationVersion()`, so Qt reports exactly `tauri-runtime-qt/` — not a
   browser string at all.

3. ChromeDriver's parser (`chrome/test/chromedriver/chrome/browser_info.cc`) requires the string to
   start with `Chrome/` or `HeadlessChrome/`, and the remainder to be **four** dot-separated
   integers whose **third (build) component is non-zero**:

   ```cpp
   static const std::string kVersionPrefix = std::string(kUserAgentProductName) + "/";
   …
   Status status = ParseBrowserVersionString(version, &browser_info->major_version, &build_no);
   if (status.IsError()) return status;
   if (build_no != 0) { … return Status(kOk); }      // 140.0.0.0 → build_no == 0 → falls through
   …
   return Status(kUnknownError, base::StringPrintf("unrecognized %s version: %s", kBrowserShortName, browser_string));
   ```

   That explains rows 1 and 2 (prefix mismatch / zero build). After a successful parse,
   `CheckVersion()` compares the parsed **major** with the driver's own ballpark
   (`kSupportedChromeMajorVersion`) — row 3. So a fix-up must produce a string that is both
   well-formed *and* major-compatible with the driver in use.

## Attached session: command battery (row 4)

`POST /session` response (abridged; full body in `probe-out-fixup-spoofed/results.json`):

```json
{"value":{"capabilities":{"browserName":"chrome","browserVersion":"155.0.8059.39",
  "chrome":{"chromedriverVersion":"155.0.8059.39 (…)"},
  "goog:chromeOptions":{"debuggerAddress":"127.0.0.1:9224"},
  "pageLoadStrategy":"normal","setWindowRect":false,
  "timeouts":{"implicit":0,"pageLoad":300000,"script":30000},
  "unhandledPromptBehavior":"dismiss and notify", …},
 "sessionId":"c6d40498833afdd8389131b2364a5484"}}
```

Note `setWindowRect:false` — ChromeDriver already advertises that this session cannot set window
bounds, consistent with the `window/rect` failures below.

39 steps, **33 pass, 6 fail**. Failures are marked ✗:

| Command (W3C endpoint) | HTTP | Measured result |
|---|---|---|
| `GET /session/{id}/url` | 200 | `"tauri://localhost/"` |
| `POST /session/{id}/url` (`tauri://localhost`) | 200 | navigation accepted (`Page.navigate`), URL still `tauri://localhost/` |
| `GET /session/{id}/title` | 200 | `"TauriQt — runtime demo"` |
| `GET /session/{id}/source` | 200 | full page HTML |
| `POST /session/{id}/execute/sync` — `return document.title` | 200 | `"TauriQt — runtime demo"` |
| `POST /session/{id}/execute/sync` — `…invoke('greet', {name:'wd-sync'})` | 200 | **`"Hello, wd-sync! — from linux"`** (promise resolved by ChromeDriver) |
| `POST /session/{id}/execute/async` — `invoke('greet', {name:'wd-async'})` | 200 | **`"Hello, wd-async! — from linux"`** |
| `POST /session/{id}/element` (css `body`) | 200 | element handle returned |
| `GET /session/{id}/element/{id}/text` | 200 | body text incl. `DEMO_OK — every probed command succeeded` |
| `POST /session/{id}/element/{id}/click` | 200 | sent as `Input.dispatchMouseEvent` ×3; page handler saw **`isTrusted=true`** |
| `GET /session/{id}/element/{id}/value` (body, input) | ✗ 404 | `unknown command: Cannot call non W3C standard command while in W3C mode` — ChromeDriver refuses this legacy endpoint by design, **no CDP command sent** |
| `GET /session/{id}/element/{id}/attribute/class` | 200 | `null` (body has no class) |
| `GET /session/{id}/element/{id}/property/value` (input) | 200 | `"wd-probe-value"` (the W3C-era value endpoint) |
| `GET /session/{id}/element/{id}/displayed` `…/enabled` | 200 | `true` / `true` |
| `POST /session/{id}/elements` (css `div`) | 200 | element array |
| `POST /session/{id}/element/{id}/clear` | 200 | input emptied |
| `POST /session/{id}/element/{id}/value` (`"wd-typed"`) | 200 | 24 accepted `Input.dispatchKeyEvent`s; input value read back as `"wd-typed"` |
| `GET /session/{id}/screenshot` | 200 | **real PNG**: `1277 × 1562`, `PNG image data, 1277 x 1562, 8-bit/color RGB, non-interlaced`, ~215 KB (214 731 bytes in the archived run; the byte count varies a few KB per run with the page's rendered text) — `tools/webdriver-probe/probe-out-fixup-spoofed/screenshot.png` |
| `GET /session/{id}/cookie` | 200 | `[]` — empty list is a legitimate answer here: the page sets no cookies, and Chromium's store answers truthfully (cf. the W4 mirror: an empty mirror is an empty list, not an error) |
| `POST /session/{id}/cookie` (with `domain: tauri.localhost` and bare) | ✗ 400 | `invalid cookie domain` — **ChromeDriver-side**; no `Network.setCookie` appears in the verbose log (read path did use `Network.getCookies`) |
| `GET`/`POST /session/{id}/window/rect` | ✗ 404 | `unknown command: 'Browser.getWindowForTarget' wasn't found` — see below |
| `GET`/`POST /session/{id}/timeouts` | 200 | defaults `{implicit:0, pageLoad:300000, script:30000}`; set accepted |
| `GET /session/{id}/window/handles` | 200 | `["D79CFF6C53A5E6B313FFB0C524551648"]` |
| `DELETE /session/{id}` | 200 | session closed, WebSocket released |

Screenshot dimensions match the demo page's own report (`win_info: inner_size=1277x1562;
outer_size=1277x1562`), i.e. `Page.captureScreenshot` returns the webview viewport 1:1.

### The one CDP method Qt rejected

From the ChromeDriver `--verbose` log (`probe-out-fixup-spoofed/chromedriver-verbose.log`),
verbatim:

```
[DEBUG]: DevTools WebSocket Response: Browser.getWindowForTarget (id=178) (session_id=) browser {"code":-32601,"message":"'Browser.getWindowForTarget' wasn't found"}
[INFO]: [<session>] RESPONSE GetWindowRect ERROR unknown command: 'Browser.getWindowForTarget' wasn't found
```

reported to the client as:

```json
{"value":{"error":"unknown command","message":"unknown command: 'Browser.getWindowForTarget' wasn't found\n  (Session info: chrome=155.0.8059.39)"}}
```

No other CDP method was rejected. (Independently measured by the T1 CDP probe:
`Browser.getWindowForTarget`/`getWindowBounds` → `-32601`.)

### What ChromeDriver actually sent over CDP (accepted by Qt)

Census from the verbose log of the attached run:

```
Page.getFrameTree 122 · Runtime.evaluate 78 · Runtime.callFunctionOn 61 · Runtime.releaseObjectGroup 42
DOM.resolveNode 41 · Input.dispatchKeyEvent 24 · Input.dispatchMouseEvent 6 · Target.getTargets 2
Target.setAutoAttach 2 · Runtime.enable 2 · Network.getCookies 2 · Browser.getWindowForTarget 2 (both -32601)
Target.attachToTarget 1 · Page.enable 1 · Page.addScriptToEvaluateOnNewDocument 1 · Log.enable 1
DOM.describeNode 1 · Page.navigate 1 · Target.activateTarget 1 · Page.captureScreenshot 1
```

So QtWebEngine's CDP is sufficient for script evaluation, DOM node resolution, target/session
plumbing, **trusted input** (mouse *and* keyboard — the page saw `isTrusted=true`), screenshots and
cookie *enumeration*.

## Fix-ups

| Option | What it does | Status |
|---|---|---|
| **(A) `/json/version` rewrite proxy** | `tools/webdriver-probe/version-fixup.mjs` serves Qt's DevTools JSON on port 9224 with `Browser` replaced; `webSocketDebuggerUrl` still points at Qt's port, so all CDP traffic stays direct (WebSocket upgrades are piped too, in case a client dials the proxy). `PROBE_FIXUP_VERSION=…` wires it into `run.sh`; `--fixup=<version>` into `probe.mjs`. | **Measured working** (row 4). One extra local process; lies about the browser version, which shows up in `session.capabilities.browserVersion` and in everything the driver gates on it. |
| **(B) Runtime-side product string** | Set `QCoreApplication::setApplicationName("Chrome")` + `setApplicationVersion("<4-part Chromium version>")` where the runtime creates `QApplication` (currently just the fake `argv0` in `qt_app.cpp`), so Qt publishes `Chrome/140.0.7339.207`. Removes the proxy *and* the parse failure, and is honest about the engine version. | Source-level only — **not implemented here** (runtime files are outside this task's ownership). Still needs a ChromeDriver whose major matches (140) or fix-up (A)/(C) on top. Note the UA already claims `Chrome/140.0.0.0`, so claiming a Chrome product string is only a small step further. |
| **(C) Version-matched ChromeDriver** | A ChromeDriver built from Chromium 140 passes `CheckVersion()`. | Does **not** fix the parse failure — Qt still says `tauri-runtime-qt/` — so (A) or (B) is required regardless. Only `chromedriver` 155 is available in this Nix store. |

Fix-up (A) as measured uses `Chrome/155.0.8059.39`; with a 140-matching driver the same proxy would
report the *truthful* `Chrome/140.0.7339.207` and need no lying at all.

## Recommendation for the test stack

**Verdict: (b) chromedriver + fix-ups** — ChromeDriver earns its keep as the WebDriver→CDP
translator; do **not** write a general WebDriver shim for the command families it already drives
correctly, and do **not** rely on it for the two families it cannot reach through Qt's CDP.

1. Ship the fix-up as part of the harness (proxy (A) now; consider (B) in the runtime later, which
   would also let Playwright/CDP users see a sane `Browser` field).
2. For Vitest/WDIO clients, the classic-protocol surface they need is covered as follows:

   | Client command (WDIO name) | W3C endpoint(s) | Measured | Who implements it |
   |---|---|---|---|
   | session | `POST /session`, `DELETE /session/{id}` | ✅ | ChromeDriver (+ fix-up A/B) |
   | url | `GET`/`POST /session/{id}/url` | ✅ | ChromeDriver |
   | title | `/title` | ✅ | ChromeDriver |
   | source | `/source` | ✅ | ChromeDriver |
   | execute sync | `/execute/sync` | ✅ | ChromeDriver |
   | execute async | `/execute/async` | ✅ | ChromeDriver |
   | element find | `/element`, `/elements` | ✅ | ChromeDriver |
   | element text | `/element/{id}/text` | ✅ | ChromeDriver |
   | element click | `/element/{id}/click` | ✅ (trusted) | ChromeDriver |
   | element value | `/element/{id}/property/value` (`/value` is legacy) | ✅ | ChromeDriver |
   | screenshot | `/screenshot` | ✅ (PNG) | ChromeDriver |
   | timeouts / implicit wait | `/timeouts` | ✅ | ChromeDriver |
   | element displayed/enabled, attribute, clear, send-keys | `/element/{id}/…` | ✅ | ChromeDriver |
   | window handles | `/window/handles` | ✅ | ChromeDriver |
   | **cookies (set)** | `POST /cookie` (read: `/cookie` ✅) | ✗ `invalid cookie domain` on `tauri://` | **shim** + T3 test-control channel (our `QWebEngineCookieStore` mirror / `set_cookie`) |
   | **window rect (get/set)** | `/window/rect` | ✗ `Browser.getWindowForTarget` missing in Qt | **shim** + T3 test-control channel (`WindowDispatch` geometry ops); Qt's CDP cannot be extended |

   So the shim's required command list for a Vitest/WDIO client is *only*: `POST /session` cookie
   handling (`POST /cookie`, and optionally `/cookie/{name}` delete and `DELETE /cookie`),
   `GET`/`POST /window/rect`, and — if we later want fullscreen/maximize/minimize convenience
   endpoints — those map onto the same window ops. Everything else in that list is already provided
   by ChromeDriver and should be forwarded to it rather than reimplemented.
3. Because cookies on a non-`http(s)` origin are refused by ChromeDriver *before* CDP, the shim
   must intercept cookie commands itself (our mirror answers them); it cannot delegate them.
4. Keep the CDP proxy (T1) separate: it is the right tool for Playwright/CDP-level analysis, while
   fix-up (A) is only about one JSON field.

## Reproduce

```sh
# one build (from the worktree root)
LD_LIBRARY_PATH= nix develop -c sh -c 'QMAKE=$(which qmake); cargo build -p tauriqt'

# 1) bare attach — fails
tools/webdriver-probe/run.sh
# 2) truthful version string — ChromeDriver's version gate fails
PROBE_FIXUP_VERSION="Chrome/140.0.7339.207" \
  PROBE_OUT="$PWD/tools/webdriver-probe/probe-out-fixup-truthful" tools/webdriver-probe/run.sh
# 3) version matching the driver — attaches; 33/39 pass
PROBE_FIXUP_VERSION="Chrome/155.0.8059.39" \
  PROBE_OUT="$PWD/tools/webdriver-probe/probe-out-fixup-spoofed" tools/webdriver-probe/run.sh
```

Each run starts its own app (CDP `9223`, killed afterwards — the runner verifies the port is free
and tears the app's process group down), starts `chromedriver --port=4444 --verbose`, and leaves
`probe-out*/{results.json,chromedriver-verbose.log,app.log,screenshot.png}`.

## Artifacts

| Path | Content |
|---|---|
| `tools/webdriver-probe/probe-out/results.json` | row 1: raw attach, verbatim 500 body, `cdpCommands: {}` |
| `tools/webdriver-probe/probe-out-fixup-truthful/results.json` | row 3: `CheckVersion()` failure |
| `tools/webdriver-probe/probe-out-fixup-spoofed/results.json` | row 4: session capabilities, all 39 steps with bodies/errors |
| `tools/webdriver-probe/probe-out-fixup-spoofed/screenshot.png` | WebDriver screenshot, PNG 1277 × 1562 |
| `tools/webdriver-probe/probe-out-fixup-spoofed/chromedriver-verbose.log` | every CDP command/response, incl. the `-32601` rejection |
| `tools/webdriver-probe/probe-out*/app.log` | app trace for the corresponding run (`… webview 1 event 4: DEMO_OK`) |

## Limits of this measurement

- One app instance, one page target, mouse/keyboard input on a single page; no frames, no
  navigation to a *different* origin (only `tauri://localhost` was exercised).
- The attached run used a **newer** driver (155) than the engine (140); with a 140 driver the CDP
  command set can only shrink, so this is the pessimistic side of that comparison.
- `window/rect` and cookie-set results are the *only* gaps observed; `Browser.*`-dependent extras
  (download behaviour, window bounds, PDF printing) are absent from Qt's CDP by design and were not
  re-measured here (see the T1 CDP matrix).
- `results.json` truncates response bodies at 2000 characters (for example `source`); the
  `chromedriver-verbose.log` next to it holds the untruncated traffic.
- The fix-up proxy is not load-tested: it forwards one HTTP GET per session plus WebSocket upgrades.


## ChromeDriver session creation against QtWebEngine (2026-10-11)

Attaching a *WebDriver-driven* suite (WebdriverIO, port override + `goog:chromeOptions.debuggerAddress`)
hits a wall the CDP-level probes never did, measured while running Readest's own e2e suite
against this runtime (`integrations/readest/e2e.sh`):

1. ChromeDriver parses `GET /json/version` and refuses QtWebEngine's browser string
   (`tauri-runtime-qt/`); the fix-up proxy must answer with **ChromeDriver's own full
   version** — `Chrome/155.0.0.0` is rejected, `Chrome/155.0.8059.39` is accepted.
2. ChromeDriver 155 does **not** call `Target.getBrowserContexts` (a stub for it is dead
   code); its first blocked call is `Target.createTarget {hidden:true, url:"about:blank"}`.
3. Answering that with the app's own page target lets session creation proceed, but
   ChromeDriver then *owns* that tab and navigates it to `about:blank` — the app page is
   gone, its `beforeunload` raises `Page.javascriptDialogOpening`, and session creation
   hangs until the client times out.

So: CDP attach works (the page the app already owns), **WebDriver does not** — the protocol
requires a target QtWebEngine cannot create.
