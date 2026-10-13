#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT OR Apache-2.0
# Run Readest on the Qt runtime, with QtWebEngine's DevTools protocol open so a
# driver can attach (this runtime has no WebDriver backend of its own — see
# README.md).
#
# Start it from this repo's dev shell, which carries Qt and qmake:
#     nix develop -c integrations/readest/run.sh
# Everything is torn down on exit; extra arguments go to the tauri CLI, e.g.
#     ... run.sh --config '{"app":{"windows":[]}}'
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checkout="${READEST_CHECKOUT:-$here/checkout}"
app="$checkout/apps/readest-app"

[[ -d "$app" ]] || {
  echo "no checkout at $app — run apply.sh first" >&2
  exit 1
}
[[ -d "$checkout/node_modules" ]] || {
  echo "no node_modules in $checkout; install deps, e.g.:" >&2
  echo "  LD_LIBRARY_PATH= nix shell nixpkgs#pnpm_11 -c pnpm install --frozen-lockfile" >&2
  exit 1
}

# The runtime's build script resolves Qt through qmake.
export QMAKE="${QMAKE:-$(command -v qmake)}"
# QtWebEngine's CDP endpoint. Chromium flags force the software path: QtWebEngine
# aborts instead of falling back when there is no GL device (see
# docs/nix-qt-gotchas.md in this repo).
export QTWEBENGINE_REMOTE_DEBUGGING="${QTWEBENGINE_REMOTE_DEBUGGING:-9222}"
export QTWEBENGINE_CHROMIUM_FLAGS="${QTWEBENGINE_CHROMIUM_FLAGS:---no-sandbox --disable-gpu --disable-gpu-compositing}"
# A build sandbox has no display; callers that have one (or a headless sway
# session) override this.
export QT_QPA_PLATFORM="${QT_QPA_PLATFORM:-offscreen}"
# Qt logs to syslog, not stderr, when stderr is not a terminal — which is
# exactly how ci.sh runs this (output redirected to a file). Without this the
# app's Qt-side output vanishes from every log the run keeps.
export QT_FORCE_STDERR_LOGGING="${QT_FORCE_STDERR_LOGGING:-1}"

cleanup() {
  [[ -n "${DEV_PID:-}" ]] && kill "$DEV_PID" 2>/dev/null || true
  [[ -n "${APP_PID:-}" ]] && kill "$APP_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Readest's beforeDevCommand is `dotenv -e .env.tauri -- next dev` and the repo
# ships no .env.tauri, so serve the frontend here and tell tauri not to (the
# same dodge their own scripts/test-tauri.sh uses).
if curl -sf --max-time 5 "http://localhost:3000" >/dev/null 2>&1; then
  echo "dev server already up on :3000"
else
  echo "starting next dev on :3000"
  (cd "$app" && exec node_modules/.bin/next dev) >"$checkout/next-dev.log" 2>&1 &
  DEV_PID=$!
  for _ in $(seq 1 120); do
    # --max-time keeps a dev server that accepts but never answers from
    # stretching a single poll (and this whole wait) without bound.
    curl -sf --max-time 5 "http://localhost:3000" >/dev/null 2>&1 && break
    kill -0 "$DEV_PID" 2>/dev/null || {
      echo "next dev died; see $checkout/next-dev.log" >&2
      exit 1
    }
    sleep 1
  done
  curl -sf --max-time 5 "http://localhost:3000" >/dev/null 2>&1 || {
    echo "next dev never became ready" >&2
    exit 1
  }
fi

echo "CDP on 127.0.0.1:$QTWEBENGINE_REMOTE_DEBUGGING (try: curl -s http://127.0.0.1:$QTWEBENGINE_REMOTE_DEBUGGING/json/version)"

cd "$app"
# Readest's own CLI wrapper (scripts/tauri.mjs) forces the cef runtime on Linux;
# for qt call the stock CLI directly.
# The CLI is a dependency of the app, not of the workspace root (their own
# wrapper resolves it the same way).
TAURI_CLI="$app/node_modules/@tauri-apps/cli/tauri.js"
[[ -f "$TAURI_CLI" ]] || {
  echo "no tauri CLI at $TAURI_CLI (deps installed?)" >&2
  exit 1
}

exec node "$TAURI_CLI" dev \
  --features qt --no-watch \
  --config '{"build":{"beforeDevCommand":""}}' \
  -- --no-default-features "$@"
