#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT OR Apache-2.0
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
# The page this integration waits for and asserts on: one substring, used by
# the readiness poll below and handed to check.mjs so the two cannot drift.
url_sub="${READEST_URL_SUBSTRING:-localhost:3000}"
# How long check.mjs may poll for the page target before giving up (the boot
# has been observed to leave the only page target un-navigated — empty url —
# for seconds after the dev server was already reachable over CDP).
page_wait_ms="${READEST_PAGE_WAIT_MS:-60000}"

fail() { echo "::error::$*" >&2; exit 1; }

# Where failure evidence is written. CI points this at the artifact's results/
# directory (apps.yml exports CI_DIAG_DIR); standalone runs get a folder beside
# the checkout.
diag_dir="${CI_DIAG_DIR:-$checkout/diagnostics}"

# Evidence for a post-mortem, gathered while the scene is still intact: what
# the DevTools port lists (every target, not just pages), what the machine's
# memory looked like, and the tails of both logs the boot writes. The nightly
# runs of 2026-10-12 and 2026-10-13 both failed with almost nothing to diagnose
# from: ci-run.log stays on the runner, next-dev.log was never uploaded, and
# Qt routes its logging to syslog when stderr is not a tty. Echoed to stderr as
# well so the job log carries it.
dump_diagnostics() {
  mkdir -p "$diag_dir"
  {
    echo "## utc"; date -u
    echo "## CDP /json/list (all targets, verbatim)"
    curl -sf --max-time 5 "http://127.0.0.1:$CDP_PORT/json/list" 2>/dev/null ||
      echo "(CDP endpoint unreachable on $CDP_PORT)"
    echo
    echo "## memory"; free -h 2>/dev/null || true
    echo "## memory pressure (PSI)"; cat /proc/pressure/memory 2>/dev/null || true
    echo "## processes by RSS"
    ps -eo pid,ppid,rss,pmem,args --sort=-rss 2>/dev/null | head -20 || true
  } >"$diag_dir/system.txt" 2>&1
  echo "== diagnostics (full set in $diag_dir) ==" >&2
  sed 's/^/  /' "$diag_dir/system.txt" >&2
  for f in "$checkout/ci-run.log" "$checkout/next-dev.log"; do
    [[ -f "$f" ]] || continue
    tail -200 "$f" >"$diag_dir/$(basename "$f").tail" 2>/dev/null || true
    echo "-- tail: $f --" >&2
    tail -40 "$f" >&2 || true
  done
}

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
  # Next's dev pipeline on this app also spawns a workerd proxy; it holds :3000
  # after the node processes are gone.
  pkill -f 'workerd' 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Up to 30 min. The app is a *grandchild* of this script (run.sh -> tauri ->
# cargo -> Readest), so `$run_pid` is not a liveness signal, and neither is the
# binary alone: the chain opens inside run.sh waiting on `next dev` (tauri is
# not exec'd yet), and later sits in the build, where there is no `Readest` yet,
# only `cargo`. Matching every phase — the wrapper script, the dev server, the
# tauri CLI, cargo, the binary — means "dead" is only reported once the whole
# chain is gone: checked twice, five seconds apart, so a short gap between
# phases is not mistaken for a crash.
chain_alive() {
  pgrep -f 'run\.sh|next dev|tauri\.js dev|target/debug/Readest|cargo run' >/dev/null
}
# Ready means: a *page* target on the DevTools port already names the dev
# server, and does so on two consecutive polls five seconds apart. The earlier
# grep matched ":3000" anywhere in /json/list — any target type, any field —
# and check.mjs then sampled the target list once, seconds later: on the
# 2026-10-13 nightly run the only page target it saw was still un-navigated
# (empty url), which failed the run even though the wait had just matched.
page_ready() {
  curl -sf --max-time 5 "http://127.0.0.1:$CDP_PORT/json/list" 2>/dev/null |
    jq -e --arg sub "$url_sub" \
      'any(.[]; .type == "page" and ((.url // "") | contains($sub)))'
}

ready=0
stable=0
for _ in $(seq 1 360); do
  if page_ready; then
    stable=$((stable + 1))
    if [[ "$stable" -ge 2 ]]; then
      ready=1
      break
    fi
  else
    stable=0
  fi
  if ! chain_alive; then
    sleep 5
    chain_alive || break
  fi
  sleep 5
done
if [[ "$ready" != "1" ]]; then
  dump_diagnostics
  fail "the app never came up on $CDP_PORT (diagnostics above; full set in $diag_dir)"
fi

echo "== assertions over CDP (the app's page, the app's commands) =="
# check.mjs polls for the page target itself now (bounded by --page-wait-ms);
# if it still fails, the dump is the post-mortem.
if ! node "$here/check.mjs" --port "$CDP_PORT" \
    --url-substring "$url_sub" --page-wait-ms "$page_wait_ms"; then
  dump_diagnostics
  fail "check.mjs failed (diagnostics above; full set in $diag_dir)"
fi

echo "== readest: all stages passed =="
