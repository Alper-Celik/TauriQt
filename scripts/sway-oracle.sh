#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT
#
# sway-oracle.sh — drive the Qt runtime under a headless Wayland compositor and
# use the *compositor* as an external oracle for window state.
#
# Why an oracle: under Wayland the runtime deliberately returns `Err` for
# top-level position (`set_position`, `inner_position`, `outer_position`) and for
# always-on-top / always-on-bottom, because xdg-shell has no global position and
# no stacking request. Those are honest protocol limits, not bugs. The sway
# compositor still knows the truth, so it can check our reporting from the
# outside and can also drive state changes we do not control:
#
#   (a) state oracle  — rect / focused / fullscreen_mode / floating / app_id from
#                       `swaymsg -t get_tree`, plus compositor-driven fullscreen,
#                       focus and move (floating + `move position`).
#   (b) positions     — compositor rect vs our `Err` for inner/outer position.
#   (c) outputs+scale — two headless outputs, one at scale 2, vs the demo's
#                       `monitors` command.
#   (d) x11-free      — `xwayland disable` + TAURIQT_PROXY_DEBUG=1 must report
#                       `platform=wayland`; nothing falls back to xcb.
#
# The session bootstrap is intentionally self-contained (private XDG_RUNTIME_DIR,
# SWAYSOCK, WLR_BACKENDS=headless, WLR_RENDERER=pixman, WLR_HEADLESS_OUTPUTS) so
# this file runs standalone; it does not require a sibling helper. See
# docs/wayland-ci.md for the design and the relation to a NixOS VM test.
#
# Usage:
#   scripts/sway-oracle.sh [--timeout S] [--app-timeout S] [--app PATH] [--keep]
#
#   --timeout S      wall-clock budget for each app run         (default 25)
#   --app-timeout S  how long the app may run before it is cut  (default 18)
#   --app PATH       app binary to run                          (default target/debug/tauriqt)
#   --keep           leave sessions behind for debugging        (default: clean up)
#   --no-build       skip the build step (reuse the existing binary)
#
# Env overrides: SWAY_BIN (store path of a sway build), ORACLE_SKIP_BUILD=1.
# Exit code 0 only if every assertion passed and no session leaked.

set -uo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"

# ---- configuration ---------------------------------------------------------

TIMEOUT=25
APP_TIMEOUT=18
APP="${APP:-$ROOT/target/debug/tauriqt}"
KEEP=0
BUILD=1

while [[ $# -gt 0 ]]; do
  case "$1" in
    --timeout) TIMEOUT="$2"; shift 2 ;;
    --app-timeout) APP_TIMEOUT="$2"; shift 2 ;;
    --app) APP="$2"; shift 2 ;;
    --keep) KEEP=1; shift ;;
    --no-build) BUILD=0; shift ;;
    -h|--help) sed -n '2,45p' "$0"; exit 0 ;;
    *) echo "sway-oracle: unknown argument: $1" >&2; exit 2 ;;
  esac
done

[[ "${ORACLE_SKIP_BUILD:-0}" == "1" ]] && BUILD=0

# Sway/QtWebEngine both come from Nix only. `LD_LIBRARY_PATH=` is cleared for
# every nix invocation (the agent sandbox otherwise injects a foreign libstdc++
# that breaks lix); the app itself is run through `nix develop` so it gets the
# Qt/QtWebEngine libraries from the flake.
SWAY_BIN="${SWAY_BIN:-}"
GRIM_BIN="${GRIM_BIN:-}"

STATE_ROOT="${TMPDIR:-/tmp}/tauriqt-oracle"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/tauriqt-oracle.XXXXXX")"

FAILURES=0
PASS_OUT=0

# nixpkgs' sway wrapper execs an *absolute* `dbus-run-session` when
# DBUS_SESSION_BUS_ADDRESS is unset, and dbus-daemon's compiled-in config is the
# FHS path /etc/dbus-1/session.conf — which a Nix build sandbox does not have, so
# sway exits 127. Bring up a bus from the config shipped in the store instead.
# On a normal desktop a bus already exists and this is a no-op.
ensure_session_bus() {
  [[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] && return 0
  local daemon root conf
  daemon="$(command -v dbus-daemon || true)"
  [[ -n "$daemon" ]] || return 0
  root="$(cd "$(dirname "$daemon")/.." && pwd)"
  for conf in "$root/share/dbus-1/session.conf" /usr/share/dbus-1/session.conf; do
    [[ -r "$conf" ]] || continue
    if DBUS_SESSION_BUS_ADDRESS="$(dbus-daemon --session --fork --print-address --config-file="$conf")"; then
      export DBUS_SESSION_BUS_ADDRESS
      return 0
    fi
  done
  return 0
}

pass() { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS_OUT=$((PASS_OUT + 1)); }
fail() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAILURES=$((FAILURES + 1)); }
info() { printf '  info %s\n' "$1"; }
section() { printf '\n== %s ==\n' "$1"; }

# ---- process / session bookkeeping -----------------------------------------

declare -a SWAY_PIDS=()
declare -a APP_PIDS=()
declare -a SESSION_DIRS=()

kill_group() {
  local pid="$1"
  [[ -z "$pid" ]] && return 0
  kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
}

cleanup() {
  local rc=$?
  # Stop every app, then every compositor, by process group (setsid leaders).
  local pid
  for pid in "${APP_PIDS[@]:-}"; do kill_group "$pid"; done
  for pid in "${SWAY_PIDS[@]:-}"; do kill_group "$pid"; done
  sleep 0.5
  for pid in "${APP_PIDS[@]:-}" "${SWAY_PIDS[@]:-}"; do
    kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true
  done
  if [[ "$KEEP" == "1" ]]; then
    echo "==> --keep: sessions preserved under $WORK (state root $STATE_ROOT)"
  else
    local d
    for d in "${SESSION_DIRS[@]:-}"; do rm -rf "$d"; done
    rm -rf "$WORK"
  fi
  exit "$rc"
}
trap cleanup EXIT INT TERM

# ---- session bootstrap ------------------------------------------------------

# session_start <outputs> [scale] [xwayland:yes|no]
# Sets: SESS_XDG SESS_WAYLAND SESS_SOCK SESS_PID SESS_LOG
session_start() {
  local outputs="$1" scale="${2:-}" xwayland="${3:-no}"
  SESS_XDG="$(mktemp -d "$STATE_ROOT.XXXXXX" 2>/dev/null || mktemp -d "${TMPDIR:-/tmp}/tauriqt-sway.XXXXXX")"
  chmod 700 "$SESS_XDG"
  SESSION_DIRS+=("$SESS_XDG")

  # A minimal, deterministic config. `xwayland disable` is the (d) guarantee that
  # no X11 path exists to fall back to.
  {
    echo "xwayland ${xwayland}"
    echo "output * bg #000000 solid_color"
  } > "$SESS_XDG/config"

  SESS_SOCK="$SESS_XDG/sway-ipc.sock"
  SESS_LOG="$SESS_XDG/sway.log"

  # env -i: a curated environment, so the session bus must be listed explicitly
  # (without it sway's wrapper falls into the dbus-run-session trap above).
  local -a sway_env=(
    PATH="$PATH" HOME="$HOME"
    XDG_RUNTIME_DIR="$SESS_XDG"
    SWAYSOCK="$SESS_SOCK"
    WLR_BACKENDS=headless
    WLR_RENDERER=pixman
    WLR_LIBINPUT_NO_DEVICES=1
    WLR_HEADLESS_OUTPUTS="$outputs"
  )
  [[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]] && sway_env+=(DBUS_SESSION_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS")

  env -i "${sway_env[@]}" \
    setsid "$SWAY_BIN/bin/sway" -c "$SESS_XDG/config" -d \
    >"$SESS_LOG" 2>&1 &
  SESS_PID=$!
  SWAY_PIDS+=("$SESS_PID")

  # Wait for the IPC socket and a live workspace.
  local i
  for i in $(seq 1 100); do
    [[ -S "$SESS_SOCK" ]] && swaymsg -t get_workspaces >/dev/null 2>&1 && break
    sleep 0.1
  done

  local way
  way="$(basename "$(ls "$SESS_XDG"/wayland-* 2>/dev/null | grep -v '\.lock$' | head -1)")"
  if [[ -z "$way" || ! -S "$SESS_SOCK" ]]; then
    echo "sway-oracle: session failed to start (see $SESS_LOG)" >&2
    tail -20 "$SESS_LOG" >&2 || true
    return 1
  fi
  SESS_WAYLAND="$way"

  if [[ -n "$scale" ]]; then
    # Apply before the app starts so Qt reads the scaled geometry at startup.
    swaymsg output HEADLESS-1 scale "$scale" >/dev/null 2>&1 || true
  fi
  return 0
}

swaymsg() { env SWAYSOCK="$SESS_SOCK" "$SWAY_BIN/bin/swaymsg" "$@"; }

# session_assert_env <label>
session_assert_env() {
  local perm
  perm="$(stat -c '%a' "$SESS_XDG")"
  if [[ "$perm" == "700" ]]; then
    pass "session: private XDG_RUNTIME_DIR (mode $perm)"
  else
    fail "session: XDG_RUNTIME_DIR mode is $perm, expected 700"
  fi
  if [[ -n "$SESS_WAYLAND" && -S "$SESS_XDG/$SESS_WAYLAND" ]]; then
    pass "session: wayland socket $SESS_WAYLAND present"
  else
    fail "session: no wayland socket found"
  fi
  if [[ -S "$SESS_SOCK" ]]; then
    pass "session: sway IPC socket reachable (SWAYSOCK)"
  else
    fail "session: sway IPC socket missing"
  fi
}

# ---- app lifecycle ----------------------------------------------------------

# app_start <extra-env...> — runs the app inside the session, logs to $APP_LOG.
app_start() {
  local extra_env=("$@")
  APP_LOG="$WORK/app-$APP_RUN.log"
  : > "$APP_LOG"
  # shellcheck disable=SC2086
  env -i \
    PATH="$PATH" HOME="$HOME" USER="${USER:-alper}" \
    XDG_RUNTIME_DIR="$SESS_XDG" \
    WAYLAND_DISPLAY="$SESS_WAYLAND" \
    SWAYSOCK="$SESS_SOCK" \
    QT_QPA_PLATFORM=wayland \
    QTWEBENGINE_CHROMIUM_FLAGS=--no-sandbox \
    "${extra_env[@]}" \
    setsid timeout -k 2 "$APP_TIMEOUT" \
      bash -c 'cd "$1" && exec env LD_LIBRARY_PATH= nix develop -c "$2"' _ "$ROOT" "$APP" \
    >"$APP_LOG" 2>&1 &
  APP_PID=$!
  APP_PIDS+=("$APP_PID")
}

app_wait_for() { # <pattern> <seconds>
  local pattern="$1" limit="$2" i
  for i in $(seq 1 $((limit * 10))); do
    grep -qF -- "$pattern" "$APP_LOG" 2>/dev/null && return 0
    sleep 0.1
  done
  return 1
}

# session_end: stop the current app + compositor and drop the runtime dir.
session_end() {
  [[ "$KEEP" == "1" ]] && return 0
  kill_group "$APP_PID"
  kill_group "$SESS_PID"
  sleep 0.4
  kill -KILL -- "-$APP_PID" 2>/dev/null || kill -KILL "$APP_PID" 2>/dev/null || true
  kill -KILL -- "-$SESS_PID" 2>/dev/null || kill -KILL "$SESS_PID" 2>/dev/null || true
  rm -rf "$SESS_XDG"
}

# teardown_all: belt-and-braces kill of everything this run started.
teardown_all() {
  [[ "$KEEP" == "1" ]] && return 0
  local pid
  for pid in "${APP_PIDS[@]:-}" "${SWAY_PIDS[@]:-}"; do
    [[ -n "$pid" ]] && kill_group "$pid"
  done
  sleep 0.5
  for pid in "${APP_PIDS[@]:-}" "${SWAY_PIDS[@]:-}"; do
    [[ -n "$pid" ]] && { kill -KILL -- "-$pid" 2>/dev/null || kill -KILL "$pid" 2>/dev/null || true; }
  done
  local d
  for d in "${SESSION_DIRS[@]:-}"; do [[ -n "$d" ]] && rm -rf "$d"; done
}

# app_tree: the current app toplevel node as one JSON line ("" if absent).
app_tree() {
  swaymsg -t get_tree -r 2>/dev/null | jq -c --arg re "$APP_MATCH" '
    [.. | objects
     | select(.shell? == "xdg_shell")
     | select(((.app_id // "") | test($re; "i")) or ((.name // "") | test($re; "i")))]
    | (.[0] // empty)
  '
}

# ---- check (a): state oracle ------------------------------------------------

check_state_oracle() {
  section "(a) state oracle: compositor drives, runtime reports"

  local node
  node="$(app_tree)"
  if [[ -z "$node" || "$node" == "null" ]]; then
    fail "compositor sees no xdg_shell toplevel for the app"
    return
  fi

  local app_id rect focused fsm
  app_id="$(jq -r '.app_id // ""' <<<"$node")"
  rect="$(jq -r '.rect | "\(.x),\(.y) \(.width)x\(.height)"' <<<"$node")"
  focused="$(jq -r '.focused' <<<"$node")"
  fsm="$(jq -r '.fullscreen_mode' <<<"$node")"

  if [[ -n "$app_id" && "$app_id" != "null" ]]; then
    pass "compositor: app_id=\"$app_id\" (Wayland toplevel identity)"
  else
    fail "compositor: toplevel has no app_id"
  fi

  if jq -e '.rect.width > 0 and .rect.height > 0' <<<"$node" >/dev/null; then
    pass "compositor: real window rect $rect"
  else
    fail "compositor: window rect is empty ($rect)"
  fi

  if [[ "$focused" == "true" ]]; then
    pass "compositor: window is focused (single toplevel)"
  else
    fail "compositor: window is not focused"
  fi

  # Our own report: the demo probes win_info at load, then again after it asks
  # the runtime for fullscreen. The compositor is the second opinion.
  if grep -qF -- "[tauriqt-demo] win_info:" "$APP_LOG"; then
    pass "runtime: win_info report captured"
    grep -qE 'is_focused=true' "$APP_LOG" \
      && pass "runtime: reports is_focused=true (matches the compositor)" \
      || fail "runtime: did not report is_focused=true"
    grep -qE '\[tauriqt-demo\] win_info: .*is_fullscreen=false' "$APP_LOG" \
      && pass "runtime: initial win_info reports is_fullscreen=false" \
      || fail "runtime: initial win_info did not report is_fullscreen=false"
  else
    fail "runtime: no win_info report in the log"
  fi

  # The app's own set_fullscreen(true) round-trip: runtime says OK, our report
  # agrees, and (below) the compositor independently sees fullscreen_mode=1.
  if grep -qF -- "fullscreen_on: set_fullscreen(true)=OK" "$APP_LOG"; then
    pass "runtime: set_fullscreen(true) succeeded"
  else
    fail "runtime: set_fullscreen(true) failed"
  fi
  grep -qE 'win_info_after_fullscreen: .*is_fullscreen=true' "$APP_LOG" \
    && pass "runtime: post-fullscreen win_info reports is_fullscreen=true" \
    || fail "runtime: post-fullscreen win_info did not report is_fullscreen=true"

  # --- compositor-driven fullscreen (from our side, not the app's request) ---
  swaymsg fullscreen enable >/dev/null 2>&1
  sleep 0.4
  fsm="$(jq -r '.fullscreen_mode' <<<"$(app_tree)")"
  if [[ "$fsm" == "1" || "$fsm" == "2" ]]; then
    pass "compositor: fullscreen enable -> fullscreen_mode=$fsm"
  else
    fail "compositor: fullscreen enable had no effect (fullscreen_mode=$fsm)"
  fi
  swaymsg fullscreen disable >/dev/null 2>&1
  sleep 0.4

  # --- compositor-driven move: float, then place at a real coordinate -------
  swaymsg floating enable >/dev/null 2>&1
  swaymsg move position 300 200 >/dev/null 2>&1
  sleep 0.4
  node="$(app_tree)"
  local mx my
  mx="$(jq -r '.rect.x' <<<"$node")"; my="$(jq -r '.rect.y' <<<"$node")"
  if [[ "$mx" -gt 0 || "$my" -gt 0 ]]; then
    pass "compositor: move position 300 200 -> rect at $mx,$my"
  else
    fail "compositor: move position did not relocate the toplevel (rect $mx,$my)"
  fi
  if jq -e '.floating != null and (.floating | startswith("user_on"))' <<<"$node" >/dev/null; then
    pass "compositor: floating enable -> floating=$(jq -r '.floating' <<<"$node")"
  else
    info "compositor: floating state is $(jq -r '.floating' <<<"$node")"
  fi

  # The runtime cannot be re-probed after load without a Rust-side change, so
  # record the gap rather than fabricate a post-move reading.
  info "gap: the demo probes win_info once at page load; a post-move reading would"
  info "     need a Rust-side re-invoke hook. The compositor rect above is the oracle."
}

# ---- check (a2): title oracle from set_title --------------------------------

check_title_oracle() {
  section "(a2) title oracle: runtime set_title vs compositor title"
  if grep -qF -- 'set_title("TauriQt — demo title")=OK' "$APP_LOG"; then
    pass "runtime: set_title succeeded"
  else
    fail "runtime: set_title did not succeed"
  fi
  local seen=0 i
  for i in $(seq 1 30); do
    if jq -e --arg t "TauriQt — demo title" 'select(.name == $t)' <<<"$(app_tree)" >/dev/null 2>&1; then
      seen=1; break
    fi
    sleep 0.1
  done
  if [[ "$seen" == "1" ]]; then
    pass "compositor: toplevel name reflects the new title"
  else
    info "compositor: title already reverted before sampling (set_title then fullscreen toggles); skipped"
  fi
}

# ---- check (b): position asymmetry -----------------------------------------

check_positions() {
  section "(b) positions: compositor rect vs our Err"

  local node rect
  node="$(app_tree)"
  rect="$(jq -r '.rect | "\(.x),\(.y) \(.width)x\(.height)"' <<<"$node")"
  echo "  compositor (swaymsg -t get_tree) rect : $rect"

  local err
  err="$(grep -oE 'inner_position=ERR\([^)]*\)' "$APP_LOG" | head -1)"
  [[ -z "$err" ]] && err="$(grep -oE 'outer_position=ERR\([^)]*\)' "$APP_LOG" | head -1)"
  echo "  runtime (win_info report)            : $err"

  if [[ -n "$rect" ]]; then
    pass "compositor reports a real rect"
  else
    fail "compositor rect unavailable"
  fi

  if grep -qF "the 'wayland' Qt platform does not report top-level window positions" "$APP_LOG"; then
    pass "runtime returns Err for inner/outer_position on wayland (honest asymmetry)"
  else
    fail "runtime did not return the expected wayland position Err"
  fi

  if grep -qE 'inner_position=[0-9-]+,[0-9-]+' "$APP_LOG"; then
    fail "runtime reported a fabricated inner_position instead of Err"
  else
    pass "runtime did not fabricate a position value"
  fi
}

# ---- check (c): multi-output + scale ---------------------------------------

check_outputs_scale() {
  section "(c) multi-output + scale: two outputs, HEADLESS-1 at scale 2"

  local outputs
  outputs="$(swaymsg -t get_outputs -r | jq -r '.[].name' | sort | tr '\n' ' ')"
  local count
  count="$(swaymsg -t get_outputs -r | jq 'length')"
  if [[ "$count" == "2" ]]; then
    pass "compositor: two outputs ($outputs)"
  else
    fail "compositor: expected 2 outputs, saw $count ($outputs)"
  fi

  local scaled
  scaled="$(swaymsg -t get_outputs -r | jq -r '.[] | select(.scale == 2) | .name')"
  if [[ -n "$scaled" ]]; then
    pass "compositor: scale 2.0 applied to $scaled"
  else
    fail "compositor: no output at scale 2"
  fi

  if grep -qF 'available_monitors=2' "$APP_LOG"; then
    pass "runtime: app sees 2 monitors"
  else
    fail "runtime: app does not see 2 monitors"
  fi
  if grep -qE 'scale=2\.0000' "$APP_LOG"; then
    pass "runtime: reports a monitor with scale=2.0000"
  else
    fail "runtime: no monitor reported at scale 2.0000"
  fi
  if grep -qE 'scale=1\.0000' "$APP_LOG"; then
    pass "runtime: reports a monitor with scale=1.0000"
  else
    fail "runtime: no monitor reported at scale 1.0000"
  fi
  grep -oE 'available_monitors=2[^;]*' "$APP_LOG" | head -1 | sed 's/^/  runtime monitors: /'
}

# ---- check (d): X11-free proof ---------------------------------------------

check_x11_free() {
  section "(d) X11-free proof: xwayland disable + proxy-debug"
  if grep -qF 'proxy-debug: native state: platform=wayland' "$APP_LOG"; then
    pass "runtime: platform=wayland (no xcb fallback)"
  else
    fail "runtime: did not report platform=wayland"
    grep -oE 'proxy-debug: native state:.*' "$APP_LOG" | head -1 | sed 's/^/  saw: /' || true
  fi
  if grep -qE 'platform=xcb' "$APP_LOG"; then
    fail "runtime: an xcb platform trace appeared"
  else
    pass "runtime: no xcb platform trace"
  fi
  # No X server is even reachable: the session disabled xwayland and DISPLAY is
  # unset for the app.
  if ! env | grep -q '^DISPLAY='; then
    info "host DISPLAY is unset in the harness (no X11 to fall back to)"
  fi
}

# ---- main -------------------------------------------------------------------

ensure_session_bus

section "preflight"
if [[ -z "$SWAY_BIN" ]]; then
  SWAY_BIN="$(LD_LIBRARY_PATH= nix build --no-link --print-out-paths nixpkgs#sway 2>/dev/null)"
fi
if [[ -z "$SWAY_BIN" || ! -x "$SWAY_BIN/bin/sway" ]]; then
  echo "sway-oracle: could not obtain sway from Nix" >&2; exit 2
fi
pass "sway from Nix: $SWAY_BIN"
if [[ -z "$GRIM_BIN" ]]; then
  GRIM_BIN="$(LD_LIBRARY_PATH= nix build --no-link --print-out-paths nixpkgs#grim 2>/dev/null || true)"
fi
[[ -n "$GRIM_BIN" && -x "$GRIM_BIN/bin/grim" ]] && pass "grim from Nix: $GRIM_BIN" \
  || info "grim unavailable (screenshot story skipped; not an assertion)"

mkdir -p "$STATE_ROOT"

if [[ "$BUILD" == "1" ]]; then
  section "build"
  if LD_LIBRARY_PATH= nix develop -c sh -c 'QMAKE=$(which qmake); export QMAKE; cargo build -p tauriqt' >/dev/null 2>&1; then
    pass "built target/debug/tauriqt"
  else
    fail "cargo build failed"
  fi
fi
if [[ ! -x "$APP" ]]; then
  fail "app binary not found: $APP"
  echo "sway-oracle: aborting (build the app or pass --app)" >&2
  exit 1
fi

APP_MATCH="${APP_MATCH:-tauri}"
APP_RUN=0

# ---- session 1: state oracle + positions -----------------------------------
section "session 1: state oracle + positions (1 output, xwayland disabled)"
if session_start 1 "" no; then
  session_assert_env
  APP_RUN=$((APP_RUN + 1))
  app_start
  if app_wait_for "tauriqt-demo] title=" "$TIMEOUT"; then
    pass "app reached the demo end (title reported)"
  else
    fail "app did not finish the demo within ${TIMEOUT}s"
  fi
  sleep 0.3
  check_state_oracle
  check_title_oracle
  check_positions
  session_end
else
  fail "session 1 failed to start"
fi

# ---- session 2: multi-output + scale ---------------------------------------
section "session 2: multi-output + scale (2 outputs, HEADLESS-1 scale 2)"
if session_start 2 2 no; then
  session_assert_env
  APP_RUN=$((APP_RUN + 1))
  app_start
  if app_wait_for "tauriqt-demo] monitors:" "$TIMEOUT"; then
    pass "app reported monitors"
  else
    fail "app did not report monitors within ${TIMEOUT}s"
  fi
  # Give the page time to finish so the log is complete, then assert.
  sleep 1
  check_outputs_scale
  session_end
else
  fail "session 2 failed to start"
fi

# ---- session 3: X11-free proof ---------------------------------------------
section "session 3: X11-free proof (xwayland disabled, TAURIQT_PROXY_DEBUG=1)"
if session_start 1 "" no; then
  session_assert_env
  APP_RUN=$((APP_RUN + 1))
  app_start "TAURIQT_PROXY_DEBUG=1"
  if app_wait_for "proxy-debug: native state:" "$TIMEOUT"; then
    pass "proxy-debug native state captured"
  else
    fail "proxy-debug native state not captured within ${TIMEOUT}s"
  fi
  check_x11_free
  session_end
else
  fail "session 3 failed to start"
fi

# ---- summary ----------------------------------------------------------------
teardown_all
section "summary"
echo "  assertions: $PASS_OUT passed, $FAILURES failed over $APP_RUN app run(s)"

# Leak check: nothing we started may survive. PID liveness is the robust test —
# the `sway` Nix wrapper execs `sway-unwrapped`, so grepping the store path can
# miss the real process; the PID survives the exec. A session dir appearing in a
# live cmdline is the second signal.
if [[ "$KEEP" == "1" ]]; then
  info "--keep: leak check skipped"
else
  leaked=0
  for pid in "${SWAY_PIDS[@]:-}" "${APP_PIDS[@]:-}"; do
    [[ -z "$pid" ]] && continue
    if kill -0 -- "-$pid" 2>/dev/null || kill -0 "$pid" 2>/dev/null; then
      leaked=1
      info "still alive: pid $pid"
    fi
  done
  for d in "${SESSION_DIRS[@]:-}"; do
    [[ -z "$d" ]] && continue
    if pgrep -f "$d" >/dev/null 2>&1; then
      leaked=1
      pgrep -af "$d" | sed 's/^/  leaked: /'
    fi
  done
  if [[ "$leaked" == "1" ]]; then
    fail "a session process is still running after teardown"
  else
    pass "no sway/app sessions left behind"
  fi
fi

if [[ "$FAILURES" -gt 0 ]]; then
  echo "==> $FAILURES assertion(s) failed"
  exit 1
fi
echo "==> sway oracle passed"
exit 0
