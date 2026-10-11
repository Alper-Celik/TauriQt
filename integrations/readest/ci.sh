#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT
#
# CI entry point for the Readest integration: prepare a pinned checkout, run
# Readest's own Rust suite against a build that links this runtime, then boot the
# app on this runtime and assert over CDP that its page rendered and its own
# `invoke()` reaches Rust.
#
# Run it from this repo's dev shell (Qt + qmake), e.g.
#   nix develop -c integrations/readest/ci.sh
# Env: READEST_CHECKOUT (default: $HOME/readest-qt-checkout, must be outside this
#      repo), READEST_CDP_PORT (9333), READEST_CI_SKIP_TESTS (1 to skip their suite).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
# CI passes the location generically (INTEGRATION_CHECKOUT); the app-specific
# name stays for local runs.
checkout="${READEST_CHECKOUT:-${INTEGRATION_CHECKOUT:-$HOME/readest-qt-checkout}}"
app="$checkout/apps/readest-app"
CDP_PORT="${READEST_CDP_PORT:-9333}"

fail() { echo "::error::$*" >&2; exit 1; }

echo "== environment =="
df -h "$checkout" 2>/dev/null | tail -1 || df -h "$(dirname "$checkout")" | tail -1

echo "== prepare the checkout (pinned commit + patch + submodules) =="
READEST_CHECKOUT="$checkout" "$here/apply.sh"

[[ -d "$checkout/node_modules" ]] || {
  echo "== install frontend deps (pnpm from nixpkgs; the repo is Nix-only) =="
  LD_LIBRARY_PATH= nix shell nixpkgs#pnpm_11 -c pnpm --dir "$checkout" install --frozen-lockfile
}

if [[ "${READEST_CI_SKIP_TESTS:-0}" != "1" ]]; then
  echo "== Readest's own Rust suite, compiled against this runtime =="
  # `--no-default-features --features qt` is the whole point: the default build is wry.
  (cd "$checkout" && cargo test -p Readest --lib --no-default-features --features qt 2>&1 | tail -5)
fi

echo "== boot the app on this runtime, with DevTools on $CDP_PORT =="
QTWEBENGINE_REMOTE_DEBUGGING="$CDP_PORT" READEST_CHECKOUT="$checkout" \
  setsid "$here/run.sh" >"$checkout/ci-run.log" 2>&1 &
run_pid=$!
cleanup() {
  # The app is a grandchild (run.sh -> tauri -> cargo -> Readest) and `setsid`
  # makes the tracked pid useless as a group to signal, so kill by pattern too:
  # leaving the binary alive holds the DevTools port and breaks the next run.
  kill -- "-$run_pid" 2>/dev/null || kill "$run_pid" 2>/dev/null || true
  pkill -f 'tauri.js dev' 2>/dev/null || true
  pkill -f 'target/debug/Readest' 2>/dev/null || true
  pkill -f 'next-server' 2>/dev/null || true
  pkill -f 'next dev' 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Up to 30 min: the first build is a full link, and the app is a *grandchild*
# of this script (run.sh -> tauri -> cargo -> Readest), so its pid is not a
# liveness signal — a vanished $run_pid used to abort the wait while the app was
# already serving. Only give up when no Readest process is left at all.
ready=0
for _ in $(seq 1 360); do
  if curl -sf "http://127.0.0.1:$CDP_PORT/json/list" 2>/dev/null | grep -q ':3000'; then
    ready=1
    break
  fi
  if ! pgrep -f 'target/debug/Readest' >/dev/null; then
    sleep 5
    pgrep -f 'target/debug/Readest' >/dev/null || break
  fi
  sleep 5
done
if [[ "$ready" != "1" ]]; then
  tail -30 "$checkout/ci-run.log" >&2 || true
  fail "the app never came up on $CDP_PORT (see the log tail above)"
fi

echo "== assertions over CDP (the app's page, the app's commands) =="
node "$here/check.mjs" --port "$CDP_PORT"

echo "== readest: all stages passed =="
