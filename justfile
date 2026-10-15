# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Dev entry points. Every recipe is a thin wrapper: the logic lives in scripts/
# and in the flake checks (nix build .#checks.<system>.<name>), which is what CI
# runs. Same three ways to run anything: bare script, `just <recipe>`,
# `nix build .#checks.<system>.<name>`.
#
# Usage:  just          (list recipes)
#         just test       (all local harnesses)
#         just check      (the flake checks = CI)

set shell := ["bash", "-euo", "pipefail", "-c"]

# The agent sandbox inherits an LD_LIBRARY_PATH that breaks nix; clearing it is a
# no-op on a normal session.
export LD_LIBRARY_PATH := ""

# nix system of this machine, e.g. aarch64-linux
system := arch() + "-linux"

# ---------------------------------------------------------------- hygiene -----

# list recipes
default:
    @just --list

# REUSE licence compliance
lint-reuse:
    reuse lint

# rustfmt + clippy over the workspace
lint-rust:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings

lint: lint-reuse lint-rust

# ------------------------------------------------------------------ build -----

# build the sample app (dev profile) through the flake's Qt environment
build:
    nix develop -c sh -c 'QMAKE=$(which qmake); cargo build -p tauriqt'

# build the packaged app (release, sandboxed, what the checks use)
build-release:
    nix build .#tauriqt

# -------------------------------------------------------------- harnesses -----

# engine smoke: app boots, tauri:// served, invoke() round-trip, demo verdict
test-smoke:
    nix develop -c scripts/smoke.sh

# the same under a headless sway session (Wayland end-to-end)
test-wayland:
    nix develop -c scripts/wayland-smoke.sh

# compositor-as-oracle: window state/geometry, multi-output + DPI, x11-free
test-oracle:
    nix develop -c scripts/sway-oracle.sh

# CDP capability matrix + Playwright attach over the stub proxy
test-cdp:
    nix develop -c node tools/cdp-probe/probe.mjs

# WebDriver: chromedriver attach + endpoint battery
test-webdriver:
    nix develop -c tools/webdriver-probe/run.sh

# every local harness (parallel: each is independent and self-reaps)
[parallel]
test: test-smoke test-wayland test-oracle

# ---------------------------------------------------------------- coverage ----

# merged llvm coverage (Rust + C++) through the harnesses: instrumented build,
# lcov.info, coverage/html (genhtml) and coverage/report (ReportGenerator)
coverage:
    nix develop .#ci -c scripts/coverage.sh

# re-render the HTML reports from the existing lcov.info (no build, no run)
coverage-html:
    nix develop .#ci -c scripts/coverage.sh --no-build --no-run

# ---------------------------------------------------------------- release -----

# package + verify-build both runtime crates, upload nothing
publish-dry:
    nix develop -c scripts/publish.sh --dry-run

# publish tauri-runtime-qt{,-core} to crates.io (needs CARGO_REGISTRY_TOKEN)
publish:
    nix develop -c scripts/publish.sh

# ------------------------------------------------------------ flake checks ----

# run one flake check by name (smoke | wayland-smoke | sway-oracle)
check name:
    nix build ".#checks.{{system}}.{{name}}" --no-link -L

# run every flake check for this system (--keep-going: one failure must not hide
# the others; each check is an independent derivation, as in the CI matrix)
check-all:
    nix build \
      ".#checks.{{system}}.smoke" \
      ".#checks.{{system}}.wayland-smoke" \
      ".#checks.{{system}}.sway-oracle" \
      --no-link -L --keep-going

# evaluate every output (fast, no builds)
check-eval:
    nix flake check --no-build --all-systems

# ------------------------------------------------------------------- ci -------

# what CI runs: hygiene + all checks
ci-all: lint-reuse check-eval check-all

# the harnesses only
ci-test: check-all
