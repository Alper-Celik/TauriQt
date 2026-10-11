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

The invoke system is not optional here: QtWebEngine never hands over custom-protocol POST
bodies, so this runtime carries the `invoke()` payload in a header. Without it every
frontend command silently fails to reach Rust.

## Running it

```bash
# 1. patch a checkout (pinned commit; delete integrations/readest/checkout to redo)
integrations/readest/apply.sh

# 2. frontend deps (once; ~2.5 min, needs network)
cd integrations/readest/checkout
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

## Driving tests

Readest's Tauri harness (`pnpm test:tauri`) starts the app with `tauri-plugin-webdriver`
and drives it through WebDriver. Their own comment is explicit that this path is
**WebKitWebDriver / wry only** — the plugin has no CEF backend, and it has no Qt one
either. So that harness cannot drive this runtime.

The route that does work is the one Readest already uses for its CEF builds: the webview's
**Chrome DevTools Protocol**. For CEF they open it with `READEST_CDP_PORT`; for QtWebEngine
it is `QTWEBENGINE_REMOTE_DEBUGGING` (both point at `127.0.0.1`), and `run.sh` sets it.

Attach with the tooling in this repo, which was written for exactly this:

- `tools/cdp-probe/` — CDP command/event matrix, and Playwright attach through a stub proxy
  (see `docs/cdp-support-matrix.md`, `docs/webdriver-attach-findings.md`)
- `tools/webdriver-probe/` — chromedriver attach + endpoint battery

```bash
curl -s http://127.0.0.1:9222/json/version   # is a CDP endpoint there at all
```

## Expectations

- Readest exercises far more of the runtime than our demo: multiple windows, window-state
  restore, plugins that touch menus/tray/updater, devtools, deep links. Gaps are likely and
  are the point of this exercise — report them, don't paper over them.
- The first Rust build is long (Readest's dependency graph plus a full Qt runtime build);
  subsequent runs are incremental.
- The patch is pinned to one Readest commit (`apply.sh` prints it). When that app moves,
  the patch is expected to rot — re-derive it against the new commit rather than
  maintaining a fork.
