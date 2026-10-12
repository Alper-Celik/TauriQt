#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Headless Wayland session manager for the Wayland-first test harness.
#
# Starts a private sway compositor per session (wlroots headless backend,
# pixman software renderer) with its own `XDG_RUNTIME_DIR`, so a machine or CI
# runner can host many independent Wayland sessions in parallel without
# touching the user's desktop session, without a GPU and without any
# runner-provided packages: sway/grim come straight from Nix.
#
# Usage:
#   scripts/sway-session.sh start [--outputs N] [--scale S] [--xwayland] [--json]
#   scripts/sway-session.sh stop <session-id|--all> [--keep]
#   scripts/sway-session.sh exec <session-id> -- <cmd...>
#   scripts/sway-session.sh list
#
# `start` prints the shell exports that join the session (diagnostics go to
# stderr), so the usual idiom is:
#   eval "$(scripts/sway-session.sh start)"
#   scripts/sway-session.sh exec "$TAURIQT_SWAY_SESSION" -- my-client
#
# State lives in the session's own runtime dir plus one small file per session
# under ${TMPDIR:-/tmp}/tauriqt-sway/ (override with TAURIQT_SWAY_STATE).
#
# Environment knobs:
#   TAURIQT_SWAY_STATE        state dir (default ${TMPDIR:-/tmp}/tauriqt-sway)
#   TAURIQT_SWAY_BOOT_TIMEOUT seconds to wait for the socket + IPC (default 25)
#   SWAY_BIN / GRIM_BIN       absolute paths to sway / grim binaries
#   TAURIQT_SWAY_PKG / TAURIQT_GRIM_PKG  store paths of those packages
#   TAURIQT_SWAY_KEEP         set to 1 to keep runtime dirs after `stop`
# (Without an override the binaries come from PATH, then from a cached store
#  path, then from `nix build nixpkgs#sway` / `nixpkgs#grim`.)

set -euo pipefail

SELF_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
CONF_TEMPLATE="$SELF_DIR/sway-ci.conf"
STATE_DIR="${TAURIQT_SWAY_STATE:-${TMPDIR:-/tmp}/tauriqt-sway}"
RUN_ROOT="$STATE_DIR/run"
STORE_CACHE="$STATE_DIR/.store"
BOOT_TIMEOUT="${TAURIQT_SWAY_BOOT_TIMEOUT:-25}"

log() { printf 'sway-session: %s\n' "$*" >&2; }
warn() { printf 'sway-session: warning: %s\n' "$*" >&2; }
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

die() {
  printf 'sway-session: error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage:
  scripts/sway-session.sh start [--outputs N] [--scale S] [--resolution WxH]
                                [--xwayland] [--json]
  scripts/sway-session.sh stop <session-id|--all> [--keep]
  scripts/sway-session.sh exec <session-id> -- <cmd...>
  scripts/sway-session.sh list
  scripts/sway-session.sh store <package> [binary]

start   Boot a private headless sway session (WLR_BACKENDS=headless,
        WLR_RENDERER=pixman, WLR_LIBINPUT_NO_DEVICES=1) in its own 0700
        XDG_RUNTIME_DIR and print the environment that joins it.
        Default output count 1, resolution 1280x720, scale 1, no XWayland.
        --json prints one JSON object instead of shell exports.
stop    Tear a session down: sway, its children and clients started through
        `exec` (idempotent; `--all` sweeps every session of this state dir).
exec    Run a command inside a live session's environment.
list    Show the sessions known to this state dir.
EOF
}

# ---------------------------------------------------------------- nix helpers

# store_bin <package> <binary> -> absolute path of the binary
#
# Resolution order (first hit wins):
#   1. SWAY_BIN / GRIM_BIN       — explicit absolute binary paths
#   2. TAURIQT_SWAY_PKG / TAURIQT_GRIM_PKG — explicit package (store) roots
#   3. PATH                      — what a CI check or devShell provides
#   4. a cached store path from an earlier run
#   5. `nix build nixpkgs#<package>` (needs network/cache access)
store_bin() {
  local pkg="$1" bin="$2" override="" path_bin="" out="" cache="$STORE_CACHE/$1"
  case "$pkg" in
  sway) override="${SWAY_BIN:-}" ;;
  grim) override="${GRIM_BIN:-}" ;;
  esac
  if [[ -n "$override" ]]; then
    [[ -x "$override" ]] || die "SWAY_BIN/GRIM_BIN is not executable: $override"
    printf '%s\n' "$override"
    return 0
  fi
  case "$pkg" in
  sway) override="${TAURIQT_SWAY_PKG:-}" ;;
  grim) override="${TAURIQT_GRIM_PKG:-}" ;;
  esac
  if [[ -n "$override" ]]; then
    [[ -x "$override/bin/$bin" ]] || die "TAURIQT_${pkg^^}_PKG has no bin/$bin: $override"
    printf '%s\n' "$override/bin/$bin"
    return 0
  fi
  if path_bin="$(command -v -- "$bin" 2>/dev/null)" && [[ -x "$path_bin" ]]; then
    printf '%s\n' "$path_bin"
    return 0
  fi
  if [[ -s "$cache" ]]; then
    out="$(<"$cache")"
    [[ -x "$out/bin/$bin" ]] && {
      printf '%s\n' "$out/bin/$bin"
      return 0
    }
  fi
  mkdir -p "$STORE_CACHE"
  out="$(LD_LIBRARY_PATH='' nix build --no-link --print-out-paths "nixpkgs#$pkg" 2>/dev/null | head -n1)" || out=""
  [[ -n "$out" && -x "$out/bin/$bin" ]] ||
    die "cannot get '$bin' from nixpkgs#$pkg (no network? pass SWAY_BIN/GRIM_BIN or TAURIQT_${pkg^^}_PKG)"
  printf '%s\n' "$out" >"$cache"
  printf '%s\n' "$out/bin/$bin"
}

# ------------------------------------------------------------------ json bits

json_str() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  printf '"%s"' "$s"
}

# --------------------------------------------------------------- state files

# load_session <id>: fills ID/PID/RUNTIME_DIR/WAYLAND_NAME/SWAYSOCK_PATH/...
load_session() {
  local id="$1" state="$STATE_DIR/$1.session" key="" value=""
  ID="" PID="" RUNTIME_DIR="" WAYLAND_NAME="" SWAYSOCK_PATH="" XWAYLAND=0
  DISPLAY_VALUE="" OUTPUTS="" SCALE=""
  if [[ -r "$state" ]]; then
    while IFS='=' read -r key value; do
      case "$key" in
      ID) ID="$value" ;;
      PID) PID="$value" ;;
      RUNTIME_DIR) RUNTIME_DIR="$value" ;;
      WAYLAND_DISPLAY) WAYLAND_NAME="$value" ;;
      SWAYSOCK) SWAYSOCK_PATH="$value" ;;
      XWAYLAND) XWAYLAND="$value" ;;
      DISPLAY_VALUE) DISPLAY_VALUE="$value" ;;
      OUTPUTS) OUTPUTS="$value" ;;
      SCALE) SCALE="$value" ;;
      esac
    done <"$state"
  fi
  [[ -n "$RUNTIME_DIR" ]] && return 0
  # State file lost: fall back to the runtime dir itself.
  if [[ -d "$RUN_ROOT/$id" ]]; then
    RUNTIME_DIR="$RUN_ROOT/$id"
    ID="$id"
    [[ -r "$RUNTIME_DIR/sway.pid" ]] && PID="$(<"$RUNTIME_DIR/sway.pid")"
    SWAYSOCK_PATH="$RUNTIME_DIR/sway-ipc.sock"
    for f in "$RUNTIME_DIR"/wayland-*; do
      [[ -S "$f" && "$f" != *.lock ]] && WAYLAND_NAME="$(basename "$f")"
    done
    return 0
  fi
  return 1
}

session_alive() { [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; }

# Process-group id of a pid, read straight from /proc/proc (no procps needed).
proc_pgid() {
  awk '{ sub(/^.*\) /, ""); split($0, a, " "); print a[3] }' "/proc/$1/stat" 2>/dev/null
}

# signal_tree <signal> <pid> — signal the whole session when `pid` really is the
# leader of its own process group (`setsid` guarantees that for sessions this
# script starts); otherwise fall back to the single process, so a stale or
# recycled pid can never take out an unrelated process group.
signal_tree() {
  local sig="$1" pid="$2"
  [[ -n "$pid" ]] || return 1
  if [[ "$(proc_pgid "$pid")" == "$pid" ]]; then
    kill -"$sig" -- "-$pid" 2>/dev/null && return 0
  fi
  kill -"$sig" "$pid" 2>/dev/null
}

# -------------------------------------------------------------------- start

# dump_log <path>: last lines of a session log, for failure diagnostics.
dump_log() {
  [[ -r "$1" ]] || return 0
  warn "last 25 lines of $1:"
  tail -n25 "$1" >&2 || true
}

wait_for_exit() {
  local pid="$1" seconds="$2" i=0 limit
  limit=$((seconds * 20))
  while ((i < limit)); do
    kill -0 "$pid" 2>/dev/null || return 0
    sleep 0.05
    i=$((i + 1))
  done
  kill -0 "$pid" 2>/dev/null && return 1
  return 0
}

cmd_start() {
  local outputs=1 scale="1" resolution="1280x720" xwayland=0 json=0
  while (($#)); do
    case "$1" in
    --outputs)
      outputs="${2:?--outputs needs a value}"
      shift 2
      ;;
    --outputs=*)
      outputs="${1#*=}"
      shift
      ;;
    --scale)
      scale="${2:?--scale needs a value}"
      shift 2
      ;;
    --scale=*)
      scale="${1#*=}"
      shift
      ;;
    --resolution)
      resolution="${2:?--resolution needs a value}"
      shift 2
      ;;
    --resolution=*)
      resolution="${1#*=}"
      shift
      ;;
    --xwayland)
      xwayland=1
      shift
      ;;
    --json)
      json=1
      shift
      ;;
    -h | --help)
      usage
      return 0
      ;;
    *) die "unknown start option: $1 (see --help)" ;;
    esac
  done

  [[ "$outputs" =~ ^[1-9][0-9]?$ ]] || die "--outputs must be 1..99 (got '$outputs')"
  [[ "$scale" =~ ^[0-9]+([.][0-9]+)?$ ]] || die "--scale must be a number (got '$scale')"
  [[ "$resolution" =~ ^[0-9]+x[0-9]+$ ]] || die "--resolution must be WxH (got '$resolution')"
  [[ -r "$CONF_TEMPLATE" ]] || die "missing config template: $CONF_TEMPLATE"

  local sway_bin swaymsg_bin
  sway_bin="$(store_bin sway sway)"
  # Keep the IPC client next to the compositor: same package, same version.
  swaymsg_bin="$(dirname -- "$sway_bin")/swaymsg"
  [[ -x "$swaymsg_bin" ]] || swaymsg_bin="$(store_bin sway swaymsg)"

  mkdir -p "$RUN_ROOT"
  local run id
  run="$(mktemp -d "$RUN_ROOT/sway-XXXXXXXX")" || die "cannot create a runtime dir under $RUN_ROOT"
  chmod 700 "$run"
  id="$(basename "$run")"

  local cfg="$run/sway.conf" log="$run/sway.log"
  {
    cat "$CONF_TEMPLATE"
    printf '\n# --- generated by scripts/sway-session.sh (%s) ---\n' "$id"
    [[ "$xwayland" == 1 ]] && printf 'xwayland enable\n'
    local i
    for ((i = 1; i <= outputs; i++)); do
      printf 'output HEADLESS-%d mode %s\n' "$i" "$resolution"
      printf 'output HEADLESS-%d scale %s\n' "$i" "$scale"
    done
  } >"$cfg"

  local swaysock="$run/sway-ipc.sock"
  # The child writes its own pid (after exec it is sway's pid), which also
  # becomes the process-group id of the whole session.
  ensure_session_bus
  env XDG_RUNTIME_DIR="$run" \
    SWAYSOCK="$swaysock" \
    WLR_BACKENDS=headless \
    WLR_RENDERER=pixman \
    WLR_LIBINPUT_NO_DEVICES=1 \
    WLR_NO_HARDWARE_CURSORS=1 \
    WLR_HEADLESS_OUTPUTS="$outputs" \
    setsid bash -c 'echo $$ >"$1"; shift; exec "$@"' _ \
    "$run/sway.pid" "$sway_bin" -c "$cfg" >>"$log" 2>&1 &

  local deadline=$((SECONDS + BOOT_TIMEOUT)) pid="" sock=""
  while [[ -z "$pid" && $SECONDS -lt $deadline ]]; do
    [[ -s "$run/sway.pid" ]] && pid="$(<"$run/sway.pid")"
    [[ -n "$pid" ]] || sleep 0.1
  done
  [[ -n "$pid" ]] || {
    dump_log "$log"
    die "'$sway_bin' did not record a pid within ${BOOT_TIMEOUT}s"
  }

  while :; do
    kill -0 "$pid" 2>/dev/null || {
      dump_log "$log"
      die "sway exited during startup (session $id, log $log)"
    }
    for f in "$run"/wayland-*; do
      [[ -S "$f" && "$f" != *.lock ]] && sock="$(basename "$f")"
    done
    if [[ -n "$sock" ]] &&
      XDG_RUNTIME_DIR="$run" SWAYSOCK="$swaysock" "$swaymsg_bin" -t get_version >/dev/null 2>&1; then
      break
    fi
    if [[ $SECONDS -ge $deadline ]]; then
      dump_log "$log"
      die "timed out after ${BOOT_TIMEOUT}s waiting for the wayland socket + sway IPC (session $id)"
    fi
    sleep 0.1
  done
  [[ -n "$sock" ]] || {
    dump_log "$log"
    die "no wayland socket appeared in $run"
  }

  export XDG_RUNTIME_DIR="$run" SWAYSOCK="$swaysock"

  # Headless outputs are requested up front (WLR_HEADLESS_OUTPUTS); ask for more
  # if the backend gave us fewer than requested (the per-output config sway
  # stored at startup is applied when the output appears).
  local have
  have="$("$swaymsg_bin" -t get_outputs 2>/dev/null | grep -c '"name": "HEADLESS' || true)"
  while ((have < outputs)); do
    "$swaymsg_bin" create_output >/dev/null 2>&1 || break
    have=$((have + 1))
  done
  ((have >= outputs)) || warn "requested $outputs outputs, compositor reports $have"

  # XWayland: find the display number of the Xwayland that belongs to this
  # session (its process group is the sway process group) and read it off the
  # X lock file, which holds Xwayland's pid.
  local display="" xwayland_failed=0
  if [[ "$xwayland" == 1 ]]; then
    local lock="" lockpid="" pgrp="" tries=0
    while [[ -z "$display" && $tries -lt 50 ]]; do
      for lock in /tmp/.X*-lock; do
        [[ -s "$lock" ]] || continue
        lockpid="$(head -n1 "$lock" 2>/dev/null | tr -dc '0-9')"
        [[ -n "$lockpid" ]] || continue
        pgrp="$(awk '{ sub(/^.*\) /, ""); split($0, a, " "); print a[3] }' "/proc/$lockpid/stat" 2>/dev/null)" || pgrp=""
        [[ "$pgrp" == "$pid" ]] || continue
        display="${lock#/tmp/.X}"
        display=":${display%-lock}"
      done
      [[ -n "$display" ]] && break
      # Xwayland could not start at all (e.g. a read-only /tmp/.X11-unix in a
      # sandbox): say so now instead of waiting out the detection loop.
      if grep -qF "Failed to start Xwayland" "$log" 2>/dev/null; then
        warn "sway could not start Xwayland (see $log); DISPLAY stays unset"
        xwayland_failed=1
        break
      fi
      sleep 0.1
      tries=$((tries + 1))
    done
    if [[ -z "$display" && "$xwayland_failed" != 1 ]]; then
      warn "XWayland requested but its display was not detected; DISPLAY stays unset"
    fi
  fi

  local state="$STATE_DIR/$id.session"
  {
    printf 'ID=%s\n' "$id"
    printf 'PID=%s\n' "$pid"
    printf 'RUNTIME_DIR=%s\n' "$run"
    printf 'WAYLAND_DISPLAY=%s\n' "$sock"
    printf 'SWAYSOCK=%s\n' "$swaysock"
    printf 'XWAYLAND=%s\n' "$xwayland"
    printf 'DISPLAY_VALUE=%s\n' "$display"
    printf 'OUTPUTS=%s\n' "$outputs"
    printf 'SCALE=%s\n' "$scale"
    printf 'RESOLUTION=%s\n' "$resolution"
    printf 'LOG=%s\n' "$log"
    printf 'CONFIG=%s\n' "$cfg"
    printf 'STARTED=%s\n' "$(date -Is)"
  } >"$state"
  chmod 600 "$state"

  if [[ "$json" == 1 ]]; then
    printf '{"session":%s,"XDG_RUNTIME_DIR":%s,"WAYLAND_DISPLAY":%s,"SWAYSOCK":%s' \
      "$(json_str "$id")" "$(json_str "$run")" "$(json_str "$sock")" "$(json_str "$swaysock")"
    printf ',"pid":%s,"outputs":%s,"scale":%s,"resolution":%s,"xwayland":%s,"display":%s' \
      "$pid" "$outputs" "$scale" "$(json_str "$resolution")" \
      "$([[ "$xwayland" == 1 ]] && echo true || echo false)" "$(json_str "$display")"
    printf ',"log":%s,"config":%s}\n' "$(json_str "$log")" "$(json_str "$cfg")"
  else
    printf 'export XDG_RUNTIME_DIR=%q\n' "$run"
    printf 'export WAYLAND_DISPLAY=%q\n' "$sock"
    printf 'export SWAYSOCK=%q\n' "$swaysock"
    printf 'export TAURIQT_SWAY_SESSION=%q\n' "$id"
    [[ -n "$display" ]] && printf 'export DISPLAY=%q\n' "$display"
    true
  fi
  log "session $id up (pid $pid, ${outputs}x output, scale $scale, xwayland $([[ "$xwayland" == 1 ]] && echo on || echo off))"
}

# --------------------------------------------------------------------- stop

# reap_clients <runtime_dir>: terminate processes still bound to the session.
# Only processes whose environment names this session's runtime dir are hit,
# never an ancestor of the current script (so a caller that exported the
# session environment for `exec` is safe) and never ourselves.
reap_clients() {
  local run="$1" pid="" pp="" p="" victims=()
  local -A ancestors=()
  p=$$
  ancestors[$p]=1
  local hop
  for ((hop = 0; hop < 64; hop++)); do
    pp="$(awk '{ sub(/^.*\) /, ""); split($0, a, " "); print a[2] }' "/proc/$p/stat" 2>/dev/null)" || pp=""
    [[ -n "$pp" && "$pp" != "0" ]] || break
    ancestors[$pp]=1
    p="$pp"
    [[ "$pp" == "1" ]] && break
  done
  local envfile
  for envfile in /proc/[0-9]*/environ; do
    [[ -r "$envfile" ]] || continue
    pid="${envfile#/proc/}"
    pid="${pid%/environ}"
    [[ -n "${ancestors[$pid]:-}" ]] && continue
    if tr '\0' '\n' <"$envfile" 2>/dev/null | grep -qxF "XDG_RUNTIME_DIR=$run"; then
      victims+=("$pid")
    fi
  done
  ((${#victims[@]})) || return 0
  warn "terminating ${#victims[@]} client(s) still attached to $run: ${victims[*]}"
  for pid in "${victims[@]}"; do
    kill -TERM "$pid" 2>/dev/null || true
  done
  sleep 0.3
  for pid in "${victims[@]}"; do
    kill -0 "$pid" 2>/dev/null && kill -KILL "$pid" 2>/dev/null || true
  done
  return 0
}

stop_session() {
  local id="$1" keep="$2"
  local state pid="" run=""
  state="$STATE_DIR/$id.session"
  if ! load_session "$id"; then
    log "no such session: $id (nothing to do)"
    return 0
  fi
  pid="$PID"
  run="$RUNTIME_DIR"

  if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
    local swaymsg_bin
    swaymsg_bin="$(store_bin sway swaymsg)"
    XDG_RUNTIME_DIR="$run" SWAYSOCK="$SWAYSOCK_PATH" "$swaymsg_bin" exit >/dev/null 2>&1 || true
    if ! wait_for_exit "$pid" 3; then
      log "sway $pid did not exit cleanly; terminating it"
      signal_tree TERM "$pid" || true
      if ! wait_for_exit "$pid" 3; then
        signal_tree KILL "$pid" || true
        wait_for_exit "$pid" 2 || warn "sway $pid survived SIGKILL"
      fi
    fi
  else
    log "session $id had no live sway process (stale state)"
  fi

  reap_clients "$run"

  # Only report the session gone once its process group is really empty (the
  # compositor exits first, its clients right after).
  local drain=0
  while ((drain < 40)); do
    [[ "$(proc_pgid "$pid")" == "$pid" ]] || break
    sleep 0.05
    drain=$((drain + 1))
  done
  ((drain < 40)) || warn "process group $pid still has members after 2s; continuing"

  if [[ "$keep" == 1 ]]; then
    log "keeping runtime dir $run (--keep)"
  else
    rm -rf -- "$run"
  fi
  rm -f -- "$state"
  log "session $id stopped"
  return 0
}

cmd_stop() {
  local target="" all=0 keep="${TAURIQT_SWAY_KEEP:-0}"
  while (($#)); do
    case "$1" in
    --all)
      all=1
      shift
      ;;
    --keep)
      keep=1
      shift
      ;;
    -h | --help)
      usage
      return 0
      ;;
    -*)
      die "unknown stop option: $1 (see --help)"
      ;;
    *)
      target="$1"
      shift
      ;;
    esac
  done

  if [[ "$all" == 1 ]]; then
    local file found=0 id
    shopt -s nullglob
    for file in "$STATE_DIR"/*.session; do
      stop_session "$(basename "${file%.session}")" "$keep" || true
      found=1
    done
    # Orphans: a runtime dir whose state file went missing (interrupted start,
    # wiped state dir). Kill whatever pid it recorded, then drop the dir.
    local dir
    for dir in "$RUN_ROOT"/*/; do
      dir="${dir%/}"
      [[ -d "$dir" ]] || continue
      if [[ -s "$dir/sway.pid" ]]; then
        id="$(basename "$dir")"
        log "cleaning orphaned runtime dir $dir"
        load_session "$id" || true
        RUNTIME_DIR="$dir"
        PID="$(<"$dir/sway.pid")"
        SWAYSOCK_PATH="$dir/sway-ipc.sock"
        if kill -0 "$PID" 2>/dev/null; then
          signal_tree TERM "$PID" || true
          wait_for_exit "$PID" 3 || signal_tree KILL "$PID" || true
        fi
        reap_clients "$dir"
      fi
      [[ "$keep" == 1 ]] || rm -rf -- "$dir"
    done
    [[ "$found" == 1 ]] || log "no sessions to stop"
    return 0
  fi

  [[ -n "$target" ]] || die "stop needs a session id or --all"
  stop_session "$target" "$keep"
}

# --------------------------------------------------------------------- exec

cmd_exec() {
  local id="${1:-}"
  [[ -n "$id" ]] || die "exec needs a session id"
  shift
  [[ "${1:-}" == "--" ]] && shift
  (($#)) || die "exec needs a command (exec <session-id> -- <cmd...>)"
  load_session "$id" || die "no such session: $id"
  session_alive || die "session $id is not running (no live pid $PID)"
  export XDG_RUNTIME_DIR="$RUNTIME_DIR"
  export WAYLAND_DISPLAY="$WAYLAND_NAME"
  export SWAYSOCK="$SWAYSOCK_PATH"
  export XDG_SESSION_TYPE=wayland
  if [[ "$XWAYLAND" == 1 && -n "$DISPLAY_VALUE" ]]; then
    export DISPLAY="$DISPLAY_VALUE"
  else
    unset DISPLAY
  fi
  exec "$@"
}

# --------------------------------------------------------------------- list

cmd_list() {
  local file id
  shopt -s nullglob
  for file in "$STATE_DIR"/*.session; do
    id="$(basename "${file%.session}")"
    load_session "$id" || continue
    local alive="dead"
    session_alive && alive="alive"
    printf '%-16s %-5s pid=%-7s outputs=%s scale=%s xwayland=%s %s\n' \
      "$ID" "$alive" "${PID:--}" "${OUTPUTS:-?}" "${SCALE:-?}" "${XWAYLAND:-?}" "$RUNTIME_DIR"
  done
  return 0
}

# --------------------------------------------------------------------- main

case "${1:-}" in
start)
  shift
  cmd_start "$@"
  ;;
stop)
  shift
  cmd_stop "$@"
  ;;
exec)
  shift
  cmd_exec "$@"
  ;;
list)
  shift
  cmd_list "$@"
  ;;
store)
  shift
  (($#)) || die "store needs <package> [binary] (e.g. store grim grim)"
  _pkg="$1"
  _bin="${2:-$1}"
  store_bin "$_pkg" "$_bin"
  ;;
-h | --help | help)
  usage
  ;;
"")
  usage >&2
  exit 2
  ;;
*)
  usage >&2
  die "unknown command: $1"
  ;;
esac
