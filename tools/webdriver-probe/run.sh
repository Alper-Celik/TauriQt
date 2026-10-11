#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT
#
# Reproducible runner for the ChromeDriver attach experiment:
#   1. build the sample app (unless PROBE_SKIP_BUILD=1 / binary exists)
#   2. start it with the DevTools port open (default 9223) under a hard timeout
#   3. run probe.mjs against that port and ChromeDriver
#   4. kill the app and ChromeDriver, always
#
# Usage:
#   tools/webdriver-probe/run.sh [--build]
#
# Env: PROBE_CDP_PORT (9223), PROBE_DRIVER_PORT (4444), PROBE_APP_TIMEOUT (120),
#      PROBE_OUT (tools/webdriver-probe/probe-out), CHROMEDRIVER (auto-detected).

set -euo pipefail

cd "$(dirname "$0")/../.."
repo_root="$PWD"
cd tools/webdriver-probe

CDP_PORT="${PROBE_CDP_PORT:-9223}"
DRIVER_PORT="${PROBE_DRIVER_PORT:-4444}"
APP_TIMEOUT="${PROBE_APP_TIMEOUT:-120}"
OUT="${PROBE_OUT:-$PWD/probe-out}"

if ! command -v chromedriver >/dev/null 2>&1; then
  CHROMEDRIVER_BIN="$(ls -d /nix/store/*chromedriver-unwrapped-*/bin/chromedriver 2>/dev/null | tail -1 || true)"
else
  CHROMEDRIVER_BIN="$(command -v chromedriver)"
fi
if [[ -z "${CHROMEDRIVER_BIN:-}" ]]; then
  echo "chromedriver not found; try: nix shell nixpkgs#chromedriver -c chromedriver --version" >&2
  exit 1
fi

if [[ ! -x "$repo_root/target/debug/tauriqt" ]]; then
  echo "==> building tauriqt (one build)"
  ( cd "$repo_root" && LD_LIBRARY_PATH= nix develop -c sh -c 'QMAKE=$(which qmake); cargo build -p tauriqt' )
fi

app_log="$(mktemp -t tauriqt-webdriver-app.XXXXXX.log)"
app_pgid=""

# Terminate the app started above, and any leftover instance of *this* repo's
# debug binary (matched by /proc/<pid>/exe, never by a loose command-line
# pattern, so a user's own tauriqt is untouched). Killing only the subshell
# would leave `nix develop`→`timeout`→`tauriqt` alive holding the CDP port.
kill_app_tree() {
  local signal="$1" exe pid
  if [[ -n "$app_pgid" ]]; then kill "-${signal}" -- "-${app_pgid}" 2>/dev/null || true; fi
  for exe in /proc/[0-9]*/exe; do
    pid="${exe#/proc/}"; pid="${pid%/exe}"
    [[ "$pid" == "$$" ]] && continue
    case "$(readlink -f "$exe" 2>/dev/null)" in
      "$repo_root/target/debug/tauriqt"|"$repo_root/target/debug/tauriqt (deleted)")
        kill "-${signal}" "$pid" 2>/dev/null || true ;;
    esac
  done
}
cleanup() {
  kill_app_tree TERM
  sleep 1
  kill_app_tree KILL
  pkill -f "chromedriver --port=${DRIVER_PORT}" 2>/dev/null || true
}
trap cleanup EXIT

# A previous run must not be able to hold the CDP port (the probe would attach
# to that instance instead of the fresh one).
if curl -sf "http://127.0.0.1:${CDP_PORT}/json/version" >/dev/null 2>&1; then
  echo "==> CDP port ${CDP_PORT} already in use; cleaning up leftovers"
  kill_app_tree TERM; sleep 1; kill_app_tree KILL
  if curl -sf "http://127.0.0.1:${CDP_PORT}/json/version" >/dev/null 2>&1; then
    echo "port ${CDP_PORT} is still busy with a process that is not this repo's tauriqt" >&2
    exit 1
  fi
fi

echo "==> starting app with CDP on ${CDP_PORT} (log: ${app_log})"
# `setsid` gives the whole nix-develop→timeout→tauriqt chain its own process
# group so cleanup can take it down in one go. `setsid` forks when it is
# already a group leader, so the group id is captured from inside the new
# session (`$$` there is the session/group leader), not from `$!`.
app_pidfile="$(mktemp -t tauriqt-webdriver-app.pid.XXXXXX)"
rm -f "$app_pidfile"
setsid sh -c "
  echo \$\$ > '$app_pidfile'
  cd '$repo_root'
  exec env LD_LIBRARY_PATH= nix develop -c sh -c \
    'QTWEBENGINE_REMOTE_DEBUGGING=${CDP_PORT} QTWEBENGINE_CHROMIUM_FLAGS=\"--no-sandbox\" timeout ${APP_TIMEOUT} ./target/debug/tauriqt'
" >"$app_log" 2>&1 &

# Wait for the DevTools HTTP endpoint to answer.
for _ in $(seq 1 120); do
  if curl -sf "http://127.0.0.1:${CDP_PORT}/json/version" >/dev/null 2>&1; then break; fi
  sleep 0.5
done
if ! curl -sf "http://127.0.0.1:${CDP_PORT}/json/version" >/dev/null 2>&1; then
  echo "app never opened CDP on ${CDP_PORT}; log tail:" >&2
  tail -30 "$app_log" >&2
  exit 1
fi

# Group id of the app chain (see the `setsid` block above); falls back to the
# /proc scan inside kill_app_tree if the file is missing.
app_pgid="$(cat "$app_pidfile" 2>/dev/null || true)"
[[ -n "$app_pgid" && "$app_pgid" =~ ^[0-9]+$ ]] || app_pgid=""
rm -f "$app_pidfile"

echo "==> running probe"
probe_args=(
  "--cdp=127.0.0.1:${CDP_PORT}"
  "--port=${DRIVER_PORT}"
  "--out=${OUT}"
  "--chromedriver=${CHROMEDRIVER_BIN}"
)
# Optional: rewrite Qt's /json/version "Browser" field through a local proxy
# (ChromeDriver refuses "tauri-runtime-qt/" — see docs/webdriver-attach-findings.md).
if [[ -n "${PROBE_FIXUP_VERSION:-}" ]]; then
  probe_args+=("--fixup=${PROBE_FIXUP_VERSION}")
  probe_args+=("--fixup-port=${PROBE_FIXUP_PORT:-9224}")
fi

node probe.mjs "${probe_args[@]}" || probe_status=$?
probe_status="${probe_status:-0}"

cp -f "$app_log" "$OUT/app.log" 2>/dev/null || true

echo "==> app trace tail (${app_log})"
grep -F "[tauri-runtime-qt]" "$app_log" | tail -20 || true

exit "$probe_status"
