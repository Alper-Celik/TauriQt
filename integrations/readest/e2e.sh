#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT
#
# Run Readest's own e2e specs against a Readest that is already running on the
# Qt runtime (integrations/readest/run.sh opens its DevTools port).
#
# Why a bridge at all: `pnpm test:e2e` normally talks to `tauri-plugin-webdriver`
# inside the app, and on Linux that plugin drives WebKitWebDriver — i.e. the wry
# runtime. To reach our runtime instead, this starts ChromeDriver against the
# app's QtWebEngine DevTools endpoint, with this repo's /json/version fix-up in
# front of it: QtWebEngine publishes its browser string as the Qt application
# name ("tauri-runtime-qt/") and ChromeDriver refuses to parse that, so the
# fix-up rewrites just that field (see tools/webdriver-probe/version-fixup.mjs).
#
# Usage: integrations/readest/e2e.sh [extra wdio args...]
# Two proxies are chained in front of the app, because ChromeDriver needs both:
#   chromedriver -> version-fixup (:9334) -> mitm (:9335) -> QtWebEngine (:9333)
# `version-fixup` fakes the Browser field of /json/version (QtWebEngine publishes
# its Qt application name there), and `mitm` stubs the handful of CDP methods Qt
# does not implement — `Target.getBrowserContexts` fails session creation
# outright with "Browser context management is not supported", and the Storage.*
# cookie commands are remapped to their Network.* equivalents.
#
# Env: READEST_CDP_PORT (9333), WDIO_DRIVER_PORT (9515), WDIO_FIXUP_PORT (9334),
#      READEST_CHECKOUT, CHROMEDRIVER (otherwise taken from nixpkgs)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
checkout="${READEST_CHECKOUT:-$(dirname "$repo")/readest-qt-checkout}"
app="$checkout/apps/readest-app"

CDP_PORT="${READEST_CDP_PORT:-9333}"
DRIVER_PORT="${WDIO_DRIVER_PORT:-9515}"
FIXUP_PORT="${WDIO_FIXUP_PORT:-9334}"

[[ -d "$app" ]] || { echo "no checkout at $app — run apply.sh first" >&2; exit 1; }
curl -sf "http://127.0.0.1:$CDP_PORT/json/version" >/dev/null || {
  echo "no DevTools endpoint on 127.0.0.1:$CDP_PORT — start the app first:" >&2
  echo "  nix develop -c integrations/readest/run.sh" >&2
  exit 1
}

CHROMEDRIVER="${CHROMEDRIVER:-}"
if [[ -z "$CHROMEDRIVER" ]]; then
  CHROMEDRIVER="$(nix build --no-link --print-out-paths nixpkgs#chromedriver)/bin/chromedriver"
fi
[[ -x "$CHROMEDRIVER" ]] || { echo "no chromedriver at $CHROMEDRIVER" >&2; exit 1; }
# The fix-up lies about the browser version on purpose, so it has to look like
# the version this ChromeDriver was built for — ChromeDriver re-reads
# /json/version itself and refuses anything it cannot parse as a Chrome build it
# knows, so use its own full version, not just the major.
driver_version="$("$CHROMEDRIVER" --version | grep -oE '[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+' | head -1)"
[[ -n "$driver_version" ]] || { echo "cannot read chromedriver version" >&2; exit 1; }

MITM_PORT="${WDIO_MITM_PORT:-9335}"
STUBS="$here/cdp-stubs.json"

cleanup() {
  [[ -n "${DRIVER_PID:-}" ]] && kill "$DRIVER_PID" 2>/dev/null || true
  [[ -n "${FIXUP_PID:-}" ]] && kill "$FIXUP_PID" 2>/dev/null || true
  [[ -n "${MITM_PID:-}" ]] && kill "$MITM_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

echo "stub proxy   : 127.0.0.1:$MITM_PORT -> 127.0.0.1:$CDP_PORT ($STUBS)"
node "$repo/tools/cdp-probe/mitm.mjs" \
  --listen "$MITM_PORT" --upstream "127.0.0.1:$CDP_PORT" \
  --log "$checkout/wdio-mitm.jsonl" --stubs "$STUBS" \
  >"$checkout/wdio-mitm.log" 2>&1 &
MITM_PID=$!

echo "fix-up proxy : 127.0.0.1:$FIXUP_PORT -> 127.0.0.1:$MITM_PORT (browser=Chrome/$driver_version)"
node "$repo/tools/webdriver-probe/version-fixup.mjs" \
  --port="$FIXUP_PORT" --target="127.0.0.1:$MITM_PORT" --browser-version="Chrome/$driver_version" \
  >"$checkout/wdio-fixup.log" 2>&1 &
FIXUP_PID=$!

echo "chromedriver : 127.0.0.1:$DRIVER_PORT ($("$CHROMEDRIVER" --version))"
"$CHROMEDRIVER" --port="$DRIVER_PORT" --verbose --log-path="$checkout/wdio-chromedriver.log" &
DRIVER_PID=$!

for _ in $(seq 1 50); do
  curl -sf "http://127.0.0.1:$DRIVER_PORT/status" >/dev/null 2>&1 && break
  sleep 0.2
done
curl -sf "http://127.0.0.1:$DRIVER_PORT/status" >/dev/null || { echo "chromedriver never became ready" >&2; exit 1; }

echo "running Readest's e2e specs through it"
cd "$app"
WDIO_PORT="$DRIVER_PORT" WDIO_DEBUGGER="127.0.0.1:$FIXUP_PORT" pnpm test:e2e "$@"
