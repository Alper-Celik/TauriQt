<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT -->

# Readest on the Qt runtime

[Readest](https://github.com/readest/readest) is a large, real Tauri 2 app — an ebook
reader with a Next.js frontend and a pile of plugins — which makes it the most demanding
test surface we have for `tauri-runtime-qt`. It is also convenient: Readest already swaps
its webview runtime by feature (`wry` on other platforms, `cef` on Linux), so plugging Qt
in means adding a `qt` arm next to `cef` rather than changing how the app works.

We keep a **patch, not a fork** (`readest-qt-runtime.patch`, against one pinned commit).
The checkout created below is disposable and gitignored.

## What the patch does

| File | Change |
|---|---|
| `apps/readest-app/src-tauri/Cargo.toml` | `qt = ["dep:tauri-runtime-qt"]`, an optional Linux dependency on this repo's runtime (path injected by `apply.sh`), mirroring the existing `cef` feature |
| `apps/readest-app/src-tauri/src/lib.rs` | `type AppRuntime = tauri_runtime_qt::QtRuntime` behind `qt`, taking precedence over `cef`/`wry`; and `builder.invoke_system(tauri_runtime_qt::INVOKE_SYSTEM_SCRIPT)` |
| `Cargo.toml` (root) | `[patch.crates-io] tauri-runtime` + `tauri-utils` → the vendored fork under `packages/tauri/crates/`. Readest's tauri family is path-vendored, so without this the graph carries **two** `tauri-runtime` 2.11.3 (vendored + crates.io) and the runtime's `Runtime` impl no longer matches the trait the app's tauri checks |

The invoke system is not optional here: QtWebEngine never hands over custom-protocol POST
bodies, so this runtime carries the `invoke()` payload in a header. Without it every
frontend command silently fails to reach Rust.

## Running it

```bash
# 1. patch a checkout (pinned commit). Lands in ../readest-qt-checkout by
#    default; READEST_CHECKOUT overrides, and it must NOT be inside this repo.
integrations/readest/apply.sh

# 2. frontend deps (once; ~2.5 min, needs network)
cd ../readest-qt-checkout
LD_LIBRARY_PATH= nix shell nixpkgs#pnpm_11 -c pnpm install --frozen-lockfile
cd -

# 3. build + run from this repo's dev shell (Qt/qmake on PATH)
nix develop -c integrations/readest/run.sh
```

`run.sh` exports `QTWEBENGINE_REMOTE_DEBUGGING=9222` and the software-GL Chromium flags,
then starts the app through the **stock** tauri CLI with `--features qt` — deliberately not
Readest's own `scripts/tauri.mjs`, which forces the `cef` runtime on Linux.

An existing checkout can be reused instead of the disposable one:
`READEST_CHECKOUT=/path/to/readest integrations/readest/apply.sh`.

Two things the checkout's location must satisfy, both learned the hard way:

- **Not inside this repo.** Our root is a cargo workspace, and a checkout nested in one
  makes cargo resolve Readest's vendored tauri fork against *our* workspace root — the
  build then dies with `workspace.package.edition was not defined`. `apply.sh` refuses
  such a path outright.
- **Several GB free.** It carries `node_modules` plus a Rust `target/` (thousands of
  crates); a small filesystem fails mid-build with `No space left on device`.

## Driving tests

Readest's Tauri harness (`pnpm test:tauri`) starts the app with `tauri-plugin-webdriver`
and drives it through WebDriver. Their own comment is explicit that this path is
**WebKitWebDriver / wry only** — the plugin has no CEF backend, and it has no Qt one
either. So that harness cannot drive this runtime.

The route that does work is the one Readest already uses for its CEF builds: the webview's
**Chrome DevTools Protocol**. For CEF they open it with `READEST_CDP_PORT`; for QtWebEngine
it is `QTWEBENGINE_REMOTE_DEBUGGING` (both point at `127.0.0.1`), and `run.sh` sets it.

Attach with the tooling in this repo, which was written for exactly this:

- `tools/cdp-probe/` — CDP command/event matrix (it batteries `invoke()` too, which is
  what proves the header-carried invoke path on a real app), and Playwright attach through
  a stub proxy (see `docs/cdp-support-matrix.md`, `docs/webdriver-attach-findings.md`)
- `tools/webdriver-probe/` — chromedriver attach + endpoint battery

```bash
curl -s http://127.0.0.1:9222/json/version      # is a CDP endpoint there at all
node tools/cdp-probe/probe.mjs --port 9222      # from this repo
pnpm cdp targets                                # Readest's own dependency-free helper
```

### Which Readest suite can actually run

| Suite | Driver | Runs on the Qt runtime? |
|---|---|---|
| `test:rust`, unit/`test:pr:web:unit`, `test:browser` | vitest (no app, or its own browser) | unaffected by the runtime |
| `test:tauri` (`scripts/test-tauri.sh`) | `tauri-plugin-webdriver`, port 4445 | **no** — on Linux that plugin drives WebKitWebDriver, and Readest's own comment says it has "no CEF backend"; it has no Qt one either |
| `test:e2e` (wdio) | WebDriver on `127.0.0.1:4445`, `browserName: chrome` | **not as configured** — same plugin endpoint |
| `test:e2e:web` (Playwright) | Playwright against the web build | not the Tauri app at all |
| CDP (`pnpm cdp`, `chromium.connectOverCDP`) | Chrome DevTools Protocol on 9222 | **yes** — this is the route |

**Measured: their wdio e2e cannot be driven on this runtime.** `e2e.sh` implements the
attempt end to end — `chromedriver -> version-fixup -> mitm (stubs) -> QtWebEngine`, with
the wdio port and debugger address made overridable in the patch — and each layer works,
but ChromeDriver's *session creation* is blocked by a Qt gap:

| Stage | Result |
|---|---|
| version fix-up | works: `Session info: chrome=155.0.8059.39` (the fake must use chromedriver's **full** version, not the major) |
| `Target.getBrowserContexts` | never sent by chromedriver 155 — the real blocker (from the traffic log) was `Target.createTarget {hidden:true, url:"about:blank"}` |
| stubbing `Target.createTarget` to return the app's own page target | session creation passes, but chromedriver **owns** the tab it "created": it navigates that target to `about:blank`, which takes the app page with it |
| consequence | the app's `beforeunload` fires (`Page.javascriptDialogOpening` on the now-blank target) and session creation hangs until wdio times out |

The root cause is the one already recorded in `docs/cdp-support-matrix.md` and
`docs/webdriver-attach-findings.md`: **QtWebEngine cannot create a second target**, and
ChromeDriver insists on a tab of its own. WebDriver-driven suites (`test:tauri`,
`test:e2e`) therefore have no path here; CDP-driven tooling does, because it attaches to the
page the app already owns (which is why `check.mjs` and `tools/cdp-probe/` work, and why
Playwright attach is limited to contexts/pages that already exist).

## Measured results (2026-10-11, aarch64/NixOS, Qt 6.11.2)

| What | Result |
|---|---|
| Readest boots on the Qt runtime | yes — their page renders (`Readest — Where You Read, Digest and Get Insight`), `window.__TAURI_INTERNALS__` present |
| `invoke()` on a real third-party app | yes — `get_environment_variable` (their command) returned the home directory through this runtime's header-carried invoke path |
| Readest's own Rust suite | `cargo test -p Readest --lib --no-default-features --features qt` → **183 passed, 0 failed** |
| CDP battery on their page | 62 commands → 55 results, 7 errors (the known `Browser.getWindowForTarget/Bounds`, `printToPDF`, browser-context gaps), 0 timeouts; 1175 events recorded |
| their wdio e2e | blocked by QtWebEngine's missing `Target.createTarget` (see above), not by the app or the port patch |

## Expectations

- Readest exercises far more of the runtime than our demo: multiple windows, window-state
  restore, plugins that touch menus/tray/updater, devtools, deep links. Gaps are likely and
  are the point of this exercise — report them, don't paper over them.
- The first Rust build is long (Readest's dependency graph plus a full Qt runtime build);
  subsequent runs are incremental.
- The patch is pinned to one Readest commit (`apply.sh` prints it). When that app moves,
  the patch is expected to rot — re-derive it against the new commit rather than
  maintaining a fork.
