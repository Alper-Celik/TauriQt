<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT -->

# TauriQt

A **Qt-based runtime for Tauri 2 on Linux**: Qt windowing replaces tao, QtWebEngine
replaces WebKitGTK (webkit2gtk). It is a custom `tauri_runtime::Runtime`, not a patch to
wry — an app picks it at the type level.

> **Fully vibe coded, in a day.** Every line in this repository — the C++ Qt side, the
> Rust runtime, the demo, the harnesses — was written by an LLM agent, driven by the
> specification the author wrote and the checks that grade it, and the whole thing landed
> in a single day (every commit in it is dated 2026-10-11). Nobody has read it line by line.
> It is working code, not reviewed code — the checks below exist precisely because the
> code was not audited by hand.

## What it is

| Piece | Contains |
|---|---|
| `tauri-runtime-qt/` | The runtime crate. Rust implements `tauri_runtime::Runtime`; `qt_*.cpp/hpp` are the Qt side (`QApplication`, `QWindow`/`QWidget`, `QWebEngineView`, URL-scheme handlers, cookies, screens), bridged with `cxx` + `qt-build-utils` |
| `tauri-runtime-qt-core/` | The Qt-free half of the runtime (cookie matching, scheme encode/decode, geometry and builder conversions). No Qt, no `cxx`, no build script — so it links and runs as a normal crate with `cargo test` |
| `tauriqt/` | The demo app — a real Tauri 2 app and the living integration test |
| `dist/` | Demo frontend (probes every command and reports a verdict) |
| `scripts/`, `tools/` | Harnesses (smoke, headless Wayland, compositor oracle, CDP, WebDriver) and coverage (`scripts/coverage.sh`) |
| `integrations/` | Other people's apps run against this runtime, their tests as the judge |
| `.github/workflows/ci.yml` | CI: eval → lint → checks → coverage, plus a gated tier for those apps |
| `flake.nix`, `justfile` | Dev environment (merged Qt prefix + qmake shim) and recipes |

Status by milestone lives in [PLAN.md](PLAN.md); the per-method Qt mapping in
[RUNTIME-TRAITS.md](RUNTIME-TRAITS.md); platform traps in
[docs/nix-qt-gotchas.md](docs/nix-qt-gotchas.md).

## Use it in your app (manual mode)

This is the only supported way in today — it is what `tauriqt/` does. Three pieces of
wiring, no runtime internals needed:

**1. Depend on it on Linux and turn tauri's default runtime off** (`tauriqt/Cargo.toml`):

```toml
[dependencies]
tauri = { version = "2", default-features = false }

[target.'cfg(target_os = "linux")'.dependencies]
tauri-runtime-qt = { path = "../TauriQt/tauri-runtime-qt" }

# keep a native runtime for the other platforms
[target.'cfg(any(target_os = "windows", target_os = "macos"))'.dependencies]
tauri = { version = "2", default-features = false, features = ["wry"] }
```

**2. Name the runtime, and pass the Qt invoke system** (`tauriqt/src/main.rs`):

```rust
#[cfg(all(target_os = "linux", not(feature = "wry")))]
pub type AppRuntime = tauri_runtime_qt::QtRuntime;
#[cfg(any(not(target_os = "linux"), feature = "wry"))]
pub type AppRuntime = tauri::Wry;

fn main() {
    let mut builder = tauri::Builder::<AppRuntime>::new()
        .invoke_handler(tauri::generate_handler![/* your commands */]);

    #[cfg(all(target_os = "linux", not(feature = "wry")))]
    {
        // QtWebEngine never hands over custom-protocol POST bodies, so this
        // runtime moves the invoke payload into a header. Without it, invoke()
        // silently fails.
        builder = builder.invoke_system(tauri_runtime_qt::INVOKE_SYSTEM_SCRIPT);
    }

    builder
        .run(tauri::generate_context!())
        .expect("error while running the tauri application");
}
```

The runtime itself is constructed by tauri through the `Runtime` trait, so nothing calls
`QtRuntime::new()` by hand. On Wayland, read `tauri_runtime_qt::platform_name()` if you
must adapt to what the platform cannot answer (below).

If your app pins an older `tauri-runtime` (a vendored 2.11, say — Readest does), take this
crate with `default-features = false`: its default `tauri-runtime-2-12` feature supplies
`WindowDispatch::set_fullscreen_on_monitor`, which only exists in 2.12 and is a
compile error against anything older.

**3. Build and run** (Qt comes from the flake, hence the qmake shim):

```bash
just build                 # nix develop -c sh -c 'QMAKE=$(which qmake); cargo build -p tauriqt'
just build-release         # nix build .#tauriqt   (packaged, release, sandboxed)
```

## Drop-in mode

**Not implemented.** There is no way to keep an existing Tauri app's code untouched and
have it run on Qt today; the manual wiring above (however small) is required.

For completeness, what a drop-in would have to be: tauri bakes its runtime into the app's
*type* — `tauri::Builder::default()` is `Builder<tauri::Wry>`, and `tauri::Wry` is an alias
for a type in `tauri-runtime-wry`. So a zero-code-change swap means shipping a crate
published under that name, exposing a `Wry<L>` type plus the per-platform webview
extension traits tauri and apps reference, and having the app select it with

```toml
[patch.crates-io]
tauri-runtime-wry = { path = "../TauriQt/tauri-runtime-wry-qt" }
```

That crate does not exist here. Upstream is moving in a compatible direction — tauri is
dropping the `wry` feature flag and `tauri::Wry` in favour of explicit runtime crates
([tauri-apps/tauri#15068](https://github.com/tauri-apps/tauri/pull/15068)) — which would
make "choose the Qt runtime" the sanctioned form of exactly this wiring.

## Verification

Everything here is graded by running the app, not by reading it. Checks are flake
checks on `aarch64-linux` (`.#checks.<system>.{smoke,wayland-smoke,sway-oracle}`) and the
same harnesses run locally through the recipes:

| Recipe | What it proves |
|---|---|
| `just test-smoke` | app boots, `tauri://` served, `invoke()` round-trip, demo verdict |
| `just test-wayland` | the same end-to-end inside a headless sway session |
| `just test-oracle` | compositor-as-oracle: window state/geometry, multi-output + DPI, X11-free |
| `just test-cdp` | CDP command matrix + Playwright attach (see docs/cdp-support-matrix.md) |
| `just test-webdriver` | chromedriver attach + WebDriver endpoint battery |
| `just test` | smoke + wayland + oracle in parallel |
| `just coverage` | instrumented build → harness runs → merged llvm lcov for Rust **and** C++, `genhtml` + ReportGenerator HTML |
| `just ci-all` / `lint` | REUSE licence lint, `cargo fmt --check`, `cargo clippy -D warnings` |

Two honest caveats, both measured rather than assumed: an environment that cannot give
QtWebEngine a GL path (no `/dev/dri`, so no QRhi, so no committed `wl_buffer`) never gets a
*mapped* window — the oracle reports that as a **skip with the evidence quoted**, fail-closed,
and `ORACLE_REQUIRE_MAPPING=1` forbids skipping (see
[docs/wayland-smoke-findings.md](docs/wayland-smoke-findings.md)); and `cargo clippy -D
warnings` still carries pre-existing findings, so that half of `lint` is not currently green.
The same harnesses run in CI for both systems.

The demo page turns the result into a `DEMO_OK` / `DEMO_PARTIAL` window title, which the
harnesses read — so the app grades itself, on the platform it is running on.

### Other people's apps

Everything above grades *our* app, which is why it is suspect by construction.
[`integrations/`](integrations/) runs other people's Tauri apps on this runtime instead, so
their code and their tests are the judge — currently
[Readest](integrations/readest/README.md): it boots on this runtime, its `invoke()` reaches
its own Rust, and **its own suite passes (183 tests)**.

The list lives in [`integrations/apps.json`](integrations/apps.json) and drives a CI job of
its own, gated to the nightly/dispatch triggers because each app is a full build. Adding an
app is a directory plus an entry in that file — see
[`integrations/README.md`](integrations/README.md) for the convention and the traps we hit
(checkout outside this cargo workspace, several GB, and prefer CDP over WebDriver: QtWebEngine
cannot create a second target, so WebDriver-driven suites are protocol-blocked).

## Known limits (structural, not bugs in progress)

- **Wayland has no global window coordinates.** A compositor owns placement, so
  `set_position` only records the position on Wayland (X11 asks the window
  manager), and `inner_position`/`outer_position` answer with Qt's record of what
  the app asked for rather than a compositor-reported value — the protocol has no
  request to read one. They never fail: callers such as
  `tauri-plugin-window-state` treat a position as always available and abort their
  startup restore on the first `Err`. `platform_name()` tells an app which
  platform it is on.
- **Invoke payloads travel in a header** (`X-Tauriqt-Payload`), because QtWebEngine never
  exposes custom-protocol request bodies — hence `INVOKE_SYSTEM_SCRIPT` above.
- Linux-only by construction; the demo falls back to wry elsewhere or with `--features wry`.
- QtWebEngine is Chromium: expect Chromium's own sandbox/GPU behaviour, and a large
  dependency (see [docs/nix-qt-gotchas.md](docs/nix-qt-gotchas.md) for the sandbox traps
  we hit).

## Licence

MIT (`LICENSES/MIT.txt`). [REUSE](https://reuse.software/) compliance is enforced by
`just lint-reuse`: every file that can hold a comment carries inline SPDX tags (markdown and
shell included), and [`REUSE.toml`](REUSE.toml) annotates only what cannot — generated
lockfiles, JSON, a binary icon, and the captured probe artifacts — each group with its reason.
