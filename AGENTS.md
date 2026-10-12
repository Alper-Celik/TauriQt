<!-- SPDX-FileCopyrightText: 2026 Alper Çelik -->
<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->

# Notes for agents working in this repository

Written after the day this repository was created, because several of these cost real time
to rediscover. Read [README.md](README.md) for what the project is; this file is how not to
fight the tooling.

## Build and verify

- Everything runs through the flake. `nix develop -c …` (or the `just` recipes, which do it
  for you) — the dev shell carries the merged-Qt prefix and the `qmake` shim the runtime's
  build script needs. `QMAKE` is set for you inside the shell.
- **This is a git flake: it pins the *committed* tree.** Commit before any `nix build` or
  the build will silently use the previous revision. The working tree stays free to leave
  `target/` and logs in.
- Smallest useful loop: `LD_LIBRARY_PATH= nix develop -c sh -c 'QMAKE=$(which qmake); cargo build -p tauriqt'`
  then `SMOKE_SKIP_BUILD=1 ./scripts/smoke.sh`. Checks: `nix build .#checks.<system>.{smoke,wayland-smoke,sway-oracle}`.
- `cargo test` only works for `tauri-runtime-qt-core` (the Qt-bearing crates cannot link a
  test binary). `cargo fmt --all` + `reuse lint` (`just lint`) before committing.

## Environment traps that will bite you

- **Disk layout is not uniform.** `/home/alper`, `/home/alper/Projects/readest` and similar
  are btrfs subvolumes with a **7.6 G limit**, while `Projects/TauriQt`, `~/.cargo` and
  `~/.omp` sit on a 570 G one. When nix starts failing with `database or disk is full` or a
  build dies with `No space left on device`, redirect the caches:
  `XDG_CACHE_HOME=/home/alper/.omp/nixcache` (and put big checkouts on the roomy volume).
- **A checkout of another cargo project must not live inside this repo.** Our root is a
  workspace; a nested checkout makes cargo resolve *that* project's vendored crates against
  our workspace root (symptom: `workspace.package.edition was not defined`).
  `integrations/readest/apply.sh` refuses such a path on purpose.
- **The nix build sandbox has no `/dev/dri`.** QtWebEngine then cannot commit a `wl_buffer`,
  so no window is ever *mapped* there even though the app runs fine. The oracle reports this
  as a fail-closed skip with the evidence; `ORACLE_REQUIRE_MAPPING=1` turns the skip into a
  failure. On a host with DRM the same harness maps the window.
- **QtWebEngine aborts instead of falling back when there is no GL device** (`qFatal` in
  `SurfaceFactoryQt::GetGLOzone`). Headless runs set
  `QTWEBENGINE_CHROMIUM_FLAGS=--no-sandbox --disable-gpu --disable-gpu-compositing` and
  `LIBGL_ALWAYS_SOFTWARE=1`; see [docs/nix-qt-gotchas.md](docs/nix-qt-gotchas.md).
- **Qt logs to syslog, not stderr**, when it has no controlling TTY and stderr is not a
  terminal — which is exactly how the harnesses run it. Export `QT_FORCE_STDERR_LOGGING=1`
  (the harnesses do) or you will conclude the runtime is silent.
- `run.sh` for an integration defaults to `QT_QPA_PLATFORM=offscreen` (headless, for CI). For
  a visible window on a real desktop, set `QT_QPA_PLATFORM=wayland` and leave the Chromium
  flags alone.

## Conventions this repository expects

- **REUSE is required and enforced** (`just lint-reuse`). Every file carries the two SPDX
  tags — the copyright one, and the licence one, whose value is `MIT OR Apache-2.0` — either
  inline (markdown gets the two-line HTML comment, shell gets it under the shebang,
  `.gitignore`/`Cargo.toml` get `#` lines) or, when the format cannot hold a comment (JSON,
  generated lockfiles, binaries, `.patch` diffs, captured probe dumps), in a `<file>.license`
  sidecar beside it. There is no `REUSE.toml`: a sidecar takes precedence over the file it
  annotates, which is also what keeps the captured CDP dumps — one embeds a DOM node whose
  text is an SPDX tag — a valid expression. Licence texts live in `LICENSES/` (`MIT.txt`,
  `Apache-2.0.txt`).
- Commits are expected as you go (this repo is agent-written; the history is the record).
  Keep the message about the behaviour, not the process.
- Docs live beside the thing they describe: `docs/*-findings.md` for measurements,
  [PLAN.md](PLAN.md) for status/milestones, [RUNTIME-TRAITS.md](RUNTIME-TRAITS.md) for the
  per-method Qt mapping, `integrations/README.md` for how other apps are added and what they
  measured. When a measurement contradicts an existing claim, fix the claim in the same
  commit.
- Report what did *not* work, with the evidence. This repository's documents are full of
  "measured: no" and that is deliberate — the harnesses cannot see everything.

## Harness contracts (don't break these)

- `TAURIQT_BIN` (a set value means "run that binary directly, never call `nix`") and `APP`
  are how the scripts find the app under test; the flake's checks set both.
- `TAURIQT_TRACE=1` makes a *release* build emit the `[tauri-runtime-qt] …` lines the
  harnesses assert on (debug builds always do). `TAURIQT_EXIT_AFTER_MS` exits the app cleanly
  so an llvm profile flushes rather than being killed.
- The demo grades itself: it writes `DEMO_OK` / `DEMO_PARTIAL…` as the window title and the
  harnesses read it, and it reports `platform_name()` alongside, so a harness can tell which
  platform produced a verdict.
- **Never make a position getter fail on Wayland.** `inner_position`/`outer_position` answer
  with Qt's record of the requested value on every platform, because callers treat a position
  as always available: `tauri-plugin-window-state`'s startup restore aborts on the first
  `Err`, and with it goes the only thing that maps the window on Linux for apps that create it
  hidden (Readest did exactly that — integrations/readest/README.md, "The hidden main window").
  Placement itself is the compositor's: `set_position` only records on Wayland, and the
  stacking hints are what get refused there.
