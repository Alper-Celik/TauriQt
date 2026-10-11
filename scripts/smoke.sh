#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT
#
# End-to-end smoke test for the Qt runtime: builds the sample app, runs it with
# a timeout, and asserts on the runtime's trace lines. Deliberately engine-level
# (no CDP/WebDriver needed) so it works anywhere the app can start — it is the
# safety net under the heavier harnesses (Playwright/WebDriver).
#
# Usage:
#   scripts/smoke.sh                      # build + run + assert
#   SMOKE_EXPECT=DEMO_OK scripts/smoke.sh # require the fully-implemented demo
#   SMOKE_SKIP_BUILD=1 scripts/smoke.sh   # reuse the existing binary
#
# Exit code 0 only if every expectation matched.

set -euo pipefail

cd "$(dirname "$0")/.."

# The runtime's traces are the harness contract; ask for them even when
# the binary under test is a release build.
export TAURIQT_TRACE="${TAURIQT_TRACE:-1}"

TIMEOUT="${SMOKE_TIMEOUT:-30}"
EXPECT="${SMOKE_EXPECT:-DEMO_OK}"
LOG="$(mktemp -t tauriqt-smoke.XXXXXX.log)"
trap 'rm -f "$LOG"' EXIT

# Three run modes, same assertions:
#   1. TAURIQT_BIN set          -> run that binary directly (Nix check / packaged app);
#      no `nix`, no LD_LIBRARY_PATH fiddling (a Nix build sandbox has neither our idiom nor our PATH).
#   2. SMOKE_SKIP_BUILD=1       -> use ./target/debug/tauriqt from the devshell.
#   3. default                  -> build first, then run.
APP="${TAURIQT_BIN:-./target/debug/tauriqt}"
CHROME_FLAGS="${QTWEBENGINE_CHROMIUM_FLAGS:---no-sandbox}"

if [[ -z "${TAURIQT_BIN:-}" && "${SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  echo "==> building tauriqt"
  LD_LIBRARY_PATH= nix develop -c sh -c 'QMAKE=$(which qmake); cargo build -p tauriqt' >/dev/null
fi

echo "==> running for ${TIMEOUT}s (log: $LOG)"
if [[ -n "${TAURIQT_BIN:-}" ]]; then
  # Direct run: no devshell, no `nix` — the wrapper binary carries its own environment.
  QTWEBENGINE_CHROMIUM_FLAGS="$CHROME_FLAGS" timeout "$TIMEOUT" "$APP" >"$LOG" 2>&1 || true
else
  # Workstation path: the app needs the Qt/Chromium environment from the flake.
  LD_LIBRARY_PATH= nix develop -c sh -c \
    "QTWEBENGINE_CHROMIUM_FLAGS=\"$CHROME_FLAGS\" timeout ${TIMEOUT} ${APP}" \
    >"$LOG" 2>&1 || true
fi

failures=0
expect() {
  local pattern="$1"
  if grep -qF -- "$pattern" "$LOG"; then
    echo "  ok   : $pattern"
  else
    echo "  FAIL : $pattern"
    failures=$((failures + 1))
  fi
}

echo "==> expectations"
expect "[tauri-runtime-qt] run: emitting Ready"
expect "] window 'main' created"
expect "] webview 'main' created"
expect "tauri GET tauri://localhost/ -> 200"
expect "ipc POST ipc://localhost/greet -> 200"
expect "$EXPECT"

# A `DEMO_PARTIAL` title, or any unimplemented path being hit, must fail the run
# even when the requested expectation matched.
expect_absent() {
  local pattern="$1"
  if grep -qF -- "$pattern" "$LOG"; then
    echo "  FAIL : must not appear: $pattern"
    failures=$((failures + 1))
  else
    echo "  ok   : absent: $pattern"
  fi
}
expect_absent "DEMO_PARTIAL"
expect_absent "is not implemented yet"

if [[ "$failures" -gt 0 ]]; then
  echo "==> ${failures} expectation(s) failed; last 40 log lines:"
  tail -40 "$LOG"
  exit 1
fi

echo "==> smoke test passed"
