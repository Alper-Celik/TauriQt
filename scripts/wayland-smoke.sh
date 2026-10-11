#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT
#
# Wayland-first end-to-end smoke test: boots a private headless sway session
# (scripts/sway-session.sh — wlroots headless backend, pixman software renderer),
# runs the sample app inside it on Qt's Wayland platform plugin, and asserts the
# same runtime traces scripts/smoke.sh checks, plus the Wayland-specific
# expectations: no X11 platform fallback, and the `Err`s Wayland makes
# unavoidable (on-top/bottom state) are *correct* behaviour. Positions are not
# among them: the compositor owns the real placement, but the runtime answers
# position queries with the value Qt recorded for the app.
#
# Usage:
#   scripts/wayland-smoke.sh [--timeout S] [--outputs N] [--scale S]
#                            [--screenshot PATH] [--proxy-debug]
#                            [--skip-build] [--keep-session]
#
# Exit code 0 only if every expectation matched.
#
# Environment:
#   TAURIQT_BIN               app binary to run (implies --skip-build; never calls
#                             nix and never touches LD_LIBRARY_PATH)
#   SMOKE_SKIP_BUILD=1        reuse the existing ./target/debug/tauriqt
#   SMOKE_TIMEOUT / WAYLAND_SMOKE_TIMEOUT   seconds for the app run (default 60)
#   TAURIQT_SWAY_BOOT_TIMEOUT seconds to wait for the compositor (default 20)
#   QTWEBENGINE_CHROMIUM_FLAGS  override the Chromium flags (default --no-sandbox,
#                             which the agent firejail requires; a missing or
#                             unwritable /dev/shm adds --disable-dev-shm-usage)
#   SWAY_BIN / GRIM_BIN       absolute store paths for sway / grim (otherwise
#                             PATH, then TAURIQT_SWAY_PKG / TAURIQT_GRIM_PKG,
#                             then `nix build nixpkgs#…`)
#   WAYLAND_SMOKE_HOME        scratch HOME for the app run (default: temp dir)
#   WAYLAND_SMOKE_KEEP=1      leave the session running on exit (debugging)
#
# The app log is kept after the run and its path printed, so a failing CI job has
# the whole trace available.

set -euo pipefail
cd "$(dirname "$0")/.."

SELF_DIR="$(pwd)/scripts"
SESSION_TOOL="$SELF_DIR/sway-session.sh"
# Invoke the sibling with the *running* interpreter: inside a Nix build sandbox
# there is no /usr/bin/env, so executing it by path would die on its shebang.
BASH_BIN="${BASH:-bash}"

# The runtime's traces are the harness contract; ask for them even when
# the binary under test is a release build.
export TAURIQT_TRACE="${TAURIQT_TRACE:-1}"

TIMEOUT="${WAYLAND_SMOKE_TIMEOUT:-${SMOKE_TIMEOUT:-60}}"
export TAURIQT_SWAY_BOOT_TIMEOUT="${TAURIQT_SWAY_BOOT_TIMEOUT:-20}"
OUTPUTS=1
SCALE=1
SCREENSHOT=""
PROXY_DEBUG="${TAURIQT_PROXY_DEBUG:+1}"
PROXY_DEBUG="${PROXY_DEBUG:-0}"
SKIP_BUILD="${SMOKE_SKIP_BUILD:-0}"
KEEP_SESSION="${WAYLAND_SMOKE_KEEP:-0}"

usage() {
  cat <<'EOF'
Usage: scripts/wayland-smoke.sh [options]

  --timeout S        seconds to let the app run (default 60, or SMOKE_TIMEOUT /
                     WAYLAND_SMOKE_TIMEOUT); the run stops early once the demo
                     page reports its verdict
  --outputs N        headless outputs in the session (default 1)
  --scale S          scale factor for those outputs (default 1)
  --screenshot PATH  also capture the output with grim and compare it with the
                     empty-session baseline (proves the client painted);
                     the baseline is written next to PATH as .baseline.png
  --proxy-debug      run with TAURIQT_PROXY_DEBUG=1 and assert the proxy probe
                     (native state, display_handle()/window_handle(), probe
                     events through the sink, clean exit 0)
  --skip-build       reuse the existing ./target/debug/tauriqt
  --keep-session     leave the sway session running after the run
  -h, --help         this text
EOF
}

while (($#)); do
  case "$1" in
  --timeout | --outputs | --scale | --screenshot)
    opt="$1"
    [[ -n "${2:-}" ]] || {
      printf 'wayland-smoke: %s needs a value\n' "$opt" >&2
      exit 2
    }
    case "$opt" in
    --timeout) TIMEOUT="$2" ;;
    --outputs) OUTPUTS="$2" ;;
    --scale) SCALE="$2" ;;
    --screenshot) SCREENSHOT="$2" ;;
    esac
    shift 2
    ;;
  --timeout=* | --outputs=* | --scale=* | --screenshot=*)
    opt="${1%%=*}"
    value="${1#*=}"
    case "$opt" in
    --timeout) TIMEOUT="$value" ;;
    --outputs) OUTPUTS="$value" ;;
    --scale) SCALE="$value" ;;
    --screenshot) SCREENSHOT="$value" ;;
    esac
    shift
    ;;
  --proxy-debug)
    PROXY_DEBUG=1
    shift
    ;;
  --skip-build)
    SKIP_BUILD=1
    shift
    ;;
  --keep-session)
    KEEP_SESSION=1
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    printf 'wayland-smoke: unknown option: %s\n' "$1" >&2
    usage >&2
    exit 2
    ;;
  esac
done

[[ "$TIMEOUT" =~ ^[1-9][0-9]*$ ]] || {
  printf 'wayland-smoke: --timeout must be a positive integer (got %s)\n' "$TIMEOUT" >&2
  exit 2
}
[[ -r "$SESSION_TOOL" ]] || {
  printf 'wayland-smoke: missing session tool %s\n' "$SESSION_TOOL" >&2
  exit 2
}

# ------------------------------------------------------------ demo acceptance

# ------------------------------------------------------------------- workspace

LOG="$(mktemp -t tauriqt-wayland-smoke.XXXXXX.log)"
RUN_HOME="${WAYLAND_SMOKE_HOME:-$(mktemp -d -t tauriqt-wayland-home.XXXXXX)}"
mkdir -p "$RUN_HOME/.local/share" "$RUN_HOME/.config" "$RUN_HOME/.cache" "$RUN_HOME/.local/state"
APP_PID_FILE="$(mktemp -u -t tauriqt-wayland-app.XXXXXX.pid)"
APP_STATUS_FILE="$(mktemp -u -t tauriqt-wayland-app.XXXXXX.status)"

SESSION_ID=""
APP_GROUP=""

cleanup() {
  local rc=$?
  if [[ -n "$APP_GROUP" ]] && kill -0 "$APP_GROUP" 2>/dev/null; then
    kill -TERM -- "-$APP_GROUP" 2>/dev/null || true
  fi
  if [[ -n "$SESSION_ID" && "$KEEP_SESSION" != 1 ]]; then
    "$BASH_BIN" "$SESSION_TOOL" stop "$SESSION_ID" >&2 || true
  elif [[ -n "$SESSION_ID" ]]; then
    printf 'wayland-smoke: keeping session %s (%s)\n' "$SESSION_ID" "$XDG_RUNTIME_DIR" >&2
  fi
  rm -f "$APP_PID_FILE" "$APP_STATUS_FILE"
  if [[ -z "${WAYLAND_SMOKE_HOME:-}" && "$KEEP_SESSION" != 1 ]]; then
    rm -rf "$RUN_HOME"
  else
    printf 'wayland-smoke: keeping scratch HOME %s\n' "$RUN_HOME" >&2
  fi
  exit "$rc"
}
trap cleanup EXIT

APP="${TAURIQT_BIN:-./target/debug/tauriqt}"
RUN_CMD=()
if [[ -n "${TAURIQT_BIN:-}" ]]; then
  [[ -x "$TAURIQT_BIN" ]] || {
    printf 'wayland-smoke: TAURIQT_BIN is not executable: %s\n' "$TAURIQT_BIN" >&2
    exit 2
  }
  SKIP_BUILD=1
  RUN_CMD=(timeout "$TIMEOUT" "$TAURIQT_BIN")
else
  # Workstation idiom: the devShell supplies the Qt/QtWebEngine environment.
  RUN_CMD=(env LD_LIBRARY_PATH='' nix develop -c sh -c 'exec "$@"' _ timeout "$TIMEOUT" "$APP")
fi

if [[ "$SKIP_BUILD" != 1 ]]; then
  echo "==> building tauriqt"
  LD_LIBRARY_PATH='' nix develop -c sh -c 'QMAKE=$(which qmake); cargo build -p tauriqt' >/dev/null
fi
[[ -x "$APP" ]] || {
  printf 'wayland-smoke: no app binary at %s (build it, or set TAURIQT_BIN)\n' "$APP" >&2
  exit 2
}

# Chromium flags. --no-sandbox is what the agent firejail needs and is harmless
# elsewhere; software rendering under pixman needs no further switches (see
# docs/wayland-smoke-findings.md).
CHROMIUM_FLAGS="${QTWEBENGINE_CHROMIUM_FLAGS:---no-sandbox}"
if [[ ! -d /dev/shm || ! -w /dev/shm ]]; then
  CHROMIUM_FLAGS="$CHROMIUM_FLAGS --disable-dev-shm-usage"
fi

# ------------------------------------------------------------------- session

echo "==> starting headless sway session (outputs=$OUTPUTS scale=$SCALE)"
eval "$("$BASH_BIN" "$SESSION_TOOL" start --outputs "$OUTPUTS" --scale "$SCALE")"
SESSION_ID="${TAURIQT_SWAY_SESSION:-}"
[[ -n "$SESSION_ID" ]] || {
  printf 'wayland-smoke: sway session did not start (no session id returned)\n' >&2
  exit 1
}
echo "    session $SESSION_ID  wayland=$WAYLAND_DISPLAY  runtime=$XDG_RUNTIME_DIR"
echo "    sway=$("$BASH_BIN" "$SESSION_TOOL" store sway)  grim=$("$BASH_BIN" "$SESSION_TOOL" store grim 2>/dev/null || echo 'unavailable')"

GRIM=""
BASELINE=""
if [[ -n "$SCREENSHOT" ]]; then
  GRIM="$("$BASH_BIN" "$SESSION_TOOL" store grim 2>/dev/null)" || GRIM=""
  [[ -x "$GRIM" ]] || {
    printf 'wayland-smoke: grim is unavailable; --screenshot needs it\n' >&2
    exit 2
  }
  BASELINE="${SCREENSHOT%.png}.baseline.png"
  "$BASH_BIN" "$SESSION_TOOL" exec "$SESSION_ID" -- "$GRIM" "$BASELINE" >&2 ||
    printf 'wayland-smoke: baseline capture failed\n' >&2
fi

# ---------------------------------------------------- environment capability

# mapping_impossible_reason — prints an evidence-backed reason when this
# environment provably cannot paint a widget window, nothing otherwise.
#
# QtWebEngine's QWebEngineView is a QQuickWidget (a render-to-texture child), so
# Qt composites the top-level window through its QRhi path ("Flushing …
# using RHI" in the `qt.widgets.painting` log) and recreates the platform window
# as an OpenGLSurface. That needs a GL/EGL backend: with no DRM device at all (a
# Nix build sandbox has none) Mesa cannot bring up EGL, Qt's compositor flush
# fails silently and the client never commits a buffer, so no pixel can reach
# the output. Fail-closed: every piece must hold, so a genuine "nothing painted"
# regression in a capable environment still fails the check.
mapping_impossible_reason() {
  [[ -n "$(ls -A /dev/dri 2>/dev/null)" ]] && return 1
  local egl rhi
  egl="$(grep -aoE 'EGL not available|Failed to initialize EGL display[^;]*' "$LOG" 2>/dev/null | head -1)"
  [[ -n "$egl" ]] || return 1
  rhi="$(grep -aoF 'Failed to create QRhi for QBackingStoreRhiSupport' "$LOG" 2>/dev/null | head -1)"
  [[ -n "$rhi" ]] || return 1
  printf '"%s" + "%s"' "$egl" "$rhi"
}

# ---------------------------------------------------------------- the app run

echo "==> running $APP for up to ${TIMEOUT}s"
echo "    chromium flags: $CHROMIUM_FLAGS"
echo "    scratch HOME:   $RUN_HOME"
echo "    log:            $LOG"

app_env=(
  -u DISPLAY
  -u DBUS_SESSION_BUS_ADDRESS
  HOME="$RUN_HOME"
  XDG_DATA_HOME="$RUN_HOME/.local/share"
  XDG_CONFIG_HOME="$RUN_HOME/.config"
  XDG_CACHE_HOME="$RUN_HOME/.cache"
  XDG_STATE_HOME="$RUN_HOME/.local/state"
  XDG_SESSION_TYPE=wayland
  QT_QPA_PLATFORM=wayland
  QTWEBENGINE_CHROMIUM_FLAGS="$CHROMIUM_FLAGS"
  # Qt's own flush decision ("… using RHI") is quoted as evidence when this
  # environment cannot paint (see mapping_impossible_reason above). The app runs
  # under `setsid` with stdio redirected — no controlling TTY — and Qt then logs
  # through syslog instead of stderr (qtbase: shouldLogToStderr()), so both
  # variables are set here rather than left to the environment.
  QT_FORCE_STDERR_LOGGING="${QT_FORCE_STDERR_LOGGING:-1}"
  QT_LOGGING_RULES="${WAYLAND_SMOKE_LOGGING_RULES:-qt.widgets.painting=true}"
)
[[ "$PROXY_DEBUG" == 1 ]] && app_env+=(TAURIQT_PROXY_DEBUG=1)

# `setsid` gives the app its own session/process group (so the whole client tree
# can be signalled at once); the child records its own pid, which doubles as the
# group id, and writes the app's exit status for the proxy-debug assertion.
env "${app_env[@]}" setsid bash -c '
  echo $$ >"$1"; shift
  status_file="$1"; shift
  "$@"
  printf "%s" "$?" >"$status_file"
' _ "$APP_PID_FILE" "$APP_STATUS_FILE" "${RUN_CMD[@]}" >"$LOG" 2>&1 &

MARKER='\[tauriqt-demo\] title=|event 4: (DEMO_OK|DEMO_PARTIAL|NO_INTERNALS)'
[[ "$PROXY_DEBUG" == 1 ]] && MARKER='proxy-debug: worker requests exit\(0\)'

STOPPED_EARLY=0
APP_STATUS=""
# Wait for the app process to appear (it records its own pid right after exec).
while [[ -z "$APP_GROUP" && $SECONDS -lt $TAURIQT_SWAY_BOOT_TIMEOUT ]]; do
  [[ -s "$APP_PID_FILE" ]] && APP_GROUP="$(<"$APP_PID_FILE")"
  [[ -n "$APP_GROUP" ]] || sleep 0.1
done
[[ -n "$APP_GROUP" ]] || {
  printf 'wayland-smoke: the app never started (log: %s)\n' "$LOG" >&2
  tail -n25 "$LOG" >&2 || true
  exit 1
}

while :; do
  if [[ "$PROXY_DEBUG" == 1 && -s "$APP_STATUS_FILE" ]]; then
    APP_STATUS="$(<"$APP_STATUS_FILE")"
    STOPPED_EARLY=1
    break
  fi
  if grep -qE -- "$MARKER" "$LOG"; then
    STOPPED_EARLY=1
    break
  fi
  kill -0 "$APP_GROUP" 2>/dev/null || break
  [[ $SECONDS -lt $((TIMEOUT + 2)) ]] || break
  sleep 0.2
done

if [[ -n "$SCREENSHOT" ]]; then
  echo "==> capturing $SCREENSHOT"
  "$BASH_BIN" "$SESSION_TOOL" exec "$SESSION_ID" -- "$GRIM" "$SCREENSHOT" >&2 ||
    printf 'wayland-smoke: screenshot capture failed\n' >&2
fi

if kill -0 "$APP_GROUP" 2>/dev/null; then
  kill -TERM -- "-$APP_GROUP" 2>/dev/null || true
  for _ in $(seq 1 40); do
    kill -0 "$APP_GROUP" 2>/dev/null || break
    sleep 0.05
  done
  kill -KILL -- "-$APP_GROUP" 2>/dev/null || true
fi
[[ -n "$APP_STATUS" ]] || APP_STATUS="$(cat "$APP_STATUS_FILE" 2>/dev/null || true)"
APP_GROUP=""

# --------------------------------------------------------------- expectations

failures=0
checks=0
skips=0
expect() {
  local pattern="$1"
  checks=$((checks + 1))
  if grep -qF -- "$pattern" "$LOG"; then
    echo "  ok   : $pattern"
  else
    echo "  FAIL : $pattern"
    failures=$((failures + 1))
  fi
}
expect_absent() {
  local pattern="$1"
  checks=$((checks + 1))
  if grep -qF -- "$pattern" "$LOG"; then
    echo "  FAIL : must not appear: $pattern"
    failures=$((failures + 1))
  else
    echo "  ok   : absent: $pattern"
  fi
}
# ERE variant, for expectations whose exact value is the platform's business
# (a position is a number, and its value is Qt's record of what the app asked).
expect_re() {
  local pattern="$1"
  checks=$((checks + 1))
  if grep -qE -- "$pattern" "$LOG"; then
    echo "  ok   : $pattern"
  else
    echo "  FAIL : $pattern"
    failures=$((failures + 1))
  fi
}
note() { printf '  info : %s\n' "$1"; }

echo "==> expectations"
expect "[tauri-runtime-qt] run: emitting Ready"
expect "] window 'main' created"
expect "] webview 'main' created"
expect "tauri GET tauri://localhost/ -> 200"
expect "ipc POST ipc://localhost/greet -> 200"
# Wayland-native: the runtime must be on the Wayland platform plugin (the demo's
# own `platform` report names it) and must never fall back to X11. Positions must
# answer with a number on every platform now — the exact value is Qt's record of
# what the app asked for (the demo asks for none), so only the shape is pinned.
expect "[tauriqt-demo] platform=wayland"
expect_re 'inner_position=[0-9-]+,[0-9-]+'
expect_re 'outer_position=[0-9-]+,[0-9-]+'
expect_absent "does not report top-level window positions"
expect_absent 'Could not load the Qt platform plugin'
expect_absent "is not implemented yet"

if [[ "$PROXY_DEBUG" == 1 ]]; then
  expect "proxy-debug: native state: platform=wayland"
  expect "proxy-debug: display_handle() = Ok(DisplayHandle(Wayland(WaylandDisplayHandle {"
  expect "proxy-debug: window_handle(u64::MAX) = Err(Unavailable)"
  expect "proxy-debug: window_handle(1) = WindowHandle(Wayland(WaylandWindowHandle {"
  expect "proxy-debug: sink received event #1: proxy-probe-1"
  expect "proxy-debug: sink received event #2: proxy-probe-2"
  expect "proxy-debug: sink received event #3: proxy-probe-3"
  expect "proxy-debug: worker requests exit(0)"
  checks=$((checks + 1))
  if [[ "$APP_STATUS" == "0" ]]; then
    echo "  ok   : the proxy-debug run exited 0"
  else
    echo "  FAIL : the proxy-debug run exited '${APP_STATUS:-<none>}' (expected 0)"
    failures=$((failures + 1))
  fi
else
  checks=$((checks + 1))
  demo_line="$(grep -F '] title=DEMO_' "$LOG" | tail -n1 || true)"
  verdict="${demo_line#*] title=}"
  case "$verdict" in
  DEMO_OK*)
    echo "  ok   : demo verdict: DEMO_OK"
    ;;
  DEMO_PARTIAL:*)
    echo "  FAIL : demo verdict: DEMO_PARTIAL — every win_info/monitors field must answer on Wayland"
    echo "         ${verdict#DEMO_PARTIAL: }"
    failures=$((failures + 1))
    ;;
  *)
    echo "  FAIL : demo verdict: no DEMO_OK/DEMO_PARTIAL title in the log"
    failures=$((failures + 1))
    ;;
  esac
  expect "ipc POST ipc://localhost/win_info -> 200"
  expect "ipc POST ipc://localhost/monitors -> 200"
fi

# ---------------------------------------------------------- optional capture

if [[ -n "$SCREENSHOT" ]]; then
  echo "==> screenshots"
  png_dims() {
    local hex
    hex="$(od -An -tx1 -j16 -N8 "$1" | tr -d ' \n')"
    printf '%sx%s' "$((16#${hex:0:8}))" "$((16#${hex:8:8}))"
  }
  checks=$((checks + 1))
  if [[ -s "$SCREENSHOT" ]]; then
    echo "  ok   : $(basename "$SCREENSHOT"): $(png_dims "$SCREENSHOT"), $(wc -c <"$SCREENSHOT") bytes"
    if [[ -s "$BASELINE" ]]; then
      if cmp -s "$BASELINE" "$SCREENSHOT"; then
        if reason="$(mapping_impossible_reason)"; then
          # An explicit, evidenced skip — not a weakened assertion: the capture
          # is stateable only because this environment cannot paint at all.
          skips=$((skips + 1))
          echo "  SKIP : capture is byte-identical to the empty-session baseline — nothing painted,"
          echo "         and this environment provably cannot paint: $reason"
          echo "         QtWebEngine's view is a QQuickWidget, so the top level flushes through"
          echo "         Qt's QRhi path, which has no backend without EGL (see"
          echo "         docs/wayland-smoke-findings.md). The paint assertion is kept and runs"
          echo "         for real wherever /dev/dri and EGL exist."
        else
          echo "  FAIL : capture is byte-identical to the empty-session baseline (nothing painted)"
          failures=$((failures + 1))
        fi
      else
        echo "  ok   : capture differs from the empty-session baseline (baseline $(basename "$BASELINE"): $(png_dims "$BASELINE"), $(wc -c <"$BASELINE") bytes) — the client painted"
      fi
    else
      echo "  ok   : (no baseline capture to compare against)"
    fi
  else
    echo "  FAIL : no screenshot written to $SCREENSHOT"
    failures=$((failures + 1))
  fi
fi

# -------------------------------------------------------------------- result

echo "==> evidence (Wayland trace)"
grep -E "proxy-debug: (native state|display_handle|window_handle)|the 'wayland' Qt platform does not report|monitors: available_monitors" "$LOG" |
  head -n6 | cut -c1-240 | sed 's/^/    /' || true

if [[ "$failures" -gt 0 ]]; then
  echo "==> ${failures} of ${checks} expectation(s) FAILED; last 30 log lines ($LOG):"
  tail -n30 "$LOG"
  exit 1
fi

if [[ "$skips" -gt 0 ]]; then
  echo "==> ${checks} expectations checked: $((checks - skips)) passed, ${skips} skipped — this environment cannot paint (see the SKIP above)"
elif [[ "$STOPPED_EARLY" == 1 ]]; then
  echo "==> ${checks} expectations passed (run stopped as soon as it reached its verdict)"
else
  echo "==> ${checks} expectations passed (the run used its full ${TIMEOUT}s)"
fi
echo "    full log: $LOG"
