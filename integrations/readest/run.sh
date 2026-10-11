#!/usr/bin/env bash
# Run Readest on the Qt runtime, with QtWebEngine's DevTools protocol open so a
# driver can attach (this runtime has no WebDriver backend of its own — see
# README.md).
#
# Start it from this repo's dev shell, which carries Qt and qmake:
#     nix develop -c integrations/readest/run.sh
# The app's own dev server is started by tauri (beforeDevCommand); stop with ^C.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checkout="${READEST_CHECKOUT:-$here/checkout}"
app="$checkout/apps/readest-app"

[[ -d "$app" ]] || { echo "no checkout at $app — run apply.sh first" >&2; exit 1; }
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

echo "CDP on 127.0.0.1:$QTWEBENGINE_REMOTE_DEBUGGING (try: curl -s \$QTWEBENGINE_REMOTE_DEBUGGING/json/version)"

cd "$app"
# The CEF build path goes through Readest's own CLI wrapper (scripts/tauri.mjs),
# which forces the cef runtime on Linux; for qt we call the stock CLI directly.
exec node "$checkout/node_modules/@tauri-apps/cli/tauri.js" dev \
  --features qt --no-watch -- --no-default-features "$@"
