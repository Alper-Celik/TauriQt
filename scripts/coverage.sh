#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT
#
# Merged code coverage for the whole stack (Rust + C++), driven by the real
# harnesses rather than `cargo test`: the runtime crate has no unit-test binary
# that links (see the `doCheck = false` note in flake.nix), so the coverage
# comes from running the sample app the way the checks run it.
#
# Pipeline — the ProjectRead.ing shape, fed by llvm coverage of *our* stack:
#   1. instrumented build: Rust with `-Cinstrument-coverage`; C++ with clang's
#      `-fprofile-instr-generate -fcoverage-mapping` (tauri-runtime-qt/build.rs,
#      gated by TAURIQT_COVERAGE=1). Both halves share one LLVM profile runtime
#      and the `LLVM_PROFILE_FILE` mechanism, so one merged profile covers both.
#   2. harness runs against that binary: scripts/smoke.sh (X11 on Xvfb) and
#      scripts/wayland-smoke.sh (headless sway), each with a distinct
#      `LLVM_PROFILE_FILE` and with `TAURIQT_EXIT_AFTER_MS` set so the app leaves
#      the event loop through `QCoreApplication::exit(0)` and the profile
#      runtime's atexit flush runs (`timeout`-killing the app would lose it).
#      The pattern is `%p` (one file per process, rewritten by every dump): the
#      runtime's `%m` merge mode re-adds the counters on each dump, which a
#      periodic dump would inflate without bound.
#   3. merge:      `llvm-profdata merge -sparse`
#   4. export:     `llvm-cov export -format=lcov` — Rust and C++ in one file
#   5. reports:    `genhtml` -> coverage/html (Nix-native, always), plus — when
#      the dotnet tool resolves — ReportGenerator -> coverage/report, the same
#      `Html;Badges` output ProjectRead.ing publishes to Cloudflare Pages.
#
# Usage:
#   scripts/coverage.sh                        # build + harnesses + reports
#   scripts/coverage.sh --no-build             # reuse the instrumented binary
#   scripts/coverage.sh --no-build --no-run    # re-render from existing profiles
#
# Environment:
#   COVERAGE_DIR             output directory (default ./coverage)
#   TAURIQT_BIN              app binary under test (default ./target/debug/tauriqt)
#   TAURIQT_EXIT_AFTER_MS    clean-exit timer for the app (default 20000)
#   TAURIQT_PROFILE_FLUSH_MS periodic profile dump of the app (default 1000; 0 = off)
#   SMOKE_TIMEOUT / WAYLAND_SMOKE_TIMEOUT   harness app-run budget (default 45)
#   COVERAGE_STRICT=1        a wayland-smoke failure is fatal (default: warning)
#   COVERAGE_DOTNET=0        skip the ReportGenerator pass
#
# Exit status: non-zero when the instrumented build fails, the X11 harness
# fails, or the run produced no instrumentation data at all.

set -euo pipefail

SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
cd "$(dirname "$SELF")/.."
ROOT="$PWD"

usage() {
  # The comment block at the top of this file is the help text.
  awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$SELF"
}

BUILD=1
RUN=1
for arg in "$@"; do
  case "$arg" in
  --no-build) BUILD=0 ;;
  --no-run) RUN=0 ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    printf 'coverage: unknown option: %s\n' "$arg" >&2
    usage >&2
    exit 2
    ;;
  esac
done

# The coverage toolchain (clang, llvm-profdata/llvm-cov/llvm-nm, genhtml,
# dotnet) lives in the flake's `ci` devShell; re-exec there when it is missing.
# IN_NIX_SHELL guards against a re-exec loop.
if [[ -z "${IN_NIX_SHELL:-}" ]] && ! command -v llvm-profdata >/dev/null 2>&1; then
  if command -v nix >/dev/null 2>&1; then
    echo "==> entering nix develop .#ci (coverage tools)"
    exec env LD_LIBRARY_PATH= nix develop ".#ci" -c bash "$SELF" "$@"
  fi
  echo "coverage: llvm-profdata is not on PATH and nix is unavailable." >&2
  echo "coverage: run this inside 'nix develop .#ci' (see flake.nix)." >&2
  exit 2
fi

COVERAGE_DIR="${COVERAGE_DIR:-$ROOT/coverage}"
RAW_DIR="$COVERAGE_DIR/raw"
LOG_DIR="$COVERAGE_DIR/logs"
PROFDATA="$COVERAGE_DIR/merged.profdata"
LCOV="$COVERAGE_DIR/lcov.info"
APP="${TAURIQT_BIN:-$ROOT/target/debug/tauriqt}"
EXIT_MS="${TAURIQT_EXIT_AFTER_MS:-20000}"
PROFILE_FLUSH_MS="${TAURIQT_PROFILE_FLUSH_MS:-1000}"
SMOKE_T="${SMOKE_TIMEOUT:-45}"
WAYLAND_T="${WAYLAND_SMOKE_TIMEOUT:-45}"

mkdir -p "$RAW_DIR" "$LOG_DIR"

# --------------------------------------------------------------- 1. build -----

if [[ "$BUILD" == 1 ]]; then
  echo "==> instrumented build (Rust -Cinstrument-coverage + clang source coverage)"
  # The C++ instrumentation is clang's (build.rs selects clang++ itself when
  # CXX is not one); say so clearly instead of failing inside the build script.
  if ! command -v clang++ >/dev/null 2>&1; then
    echo "coverage: clang++ is not on PATH (needed for the C++ instrumentation)" >&2
    echo "coverage: run inside 'nix develop .#ci' (see flake.nix)" >&2
    exit 2
  fi
  # A fresh run must not inherit last run's raw profiles (they would double the
  # hit counts); with --no-run the existing profiles are the point, so keep them.
  if [[ "$RUN" == 1 ]]; then
    rm -f "$RAW_DIR"/*.profraw
  fi
  export TAURIQT_COVERAGE=1
  export CARGO_INCREMENTAL=0
  # Cargo compiles build scripts with the same flags and *runs* them during the
  # build: with no LLVM_PROFILE_FILE they would each drop a `default*.profraw`
  # into their crate directory. Park those dumps next to the report instead —
  # they are build glue, not the runtime under test, and the merge only looks at
  # coverage/raw/ — so the checkout stays clean.
  rm -rf "$COVERAGE_DIR/build-profiles"
  mkdir -p "$COVERAGE_DIR/build-profiles"
  export LLVM_PROFILE_FILE="$COVERAGE_DIR/build-profiles/build-%p.profraw"
  case " ${RUSTFLAGS:-} " in
  *" -Cinstrument-coverage "* | *" -C instrument-coverage "*) ;;
  *) export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }-Cinstrument-coverage" ;;
  esac
  # `QMAKE` is re-resolved from PATH: the shell's value may point at qtbase's
  # own qmake rather than the merged-tree shim (see flake.nix, qmakeMerged).
  QMAKE="$(command -v qmake)" cargo build -p tauriqt
else
  echo "==> reusing the existing binary (--no-build)"
fi

[[ -x "$APP" ]] || {
  echo "coverage: no app binary at $APP (drop --no-build)" >&2
  exit 2
}

# `--no-build` must not silently report coverage from a normal build; a binary
# with the profile runtime linked in is the instrumented one. (`grep -c` so the
# pipeline cannot die of SIGPIPE from an early `-q` exit.)
if command -v llvm-nm >/dev/null 2>&1; then
  profile_symbols="$(llvm-nm "$APP" 2>/dev/null | grep -c '__llvm_profile_write_file' || true)"
  if [[ "$profile_symbols" == 0 ]]; then
    echo "coverage: $APP has no LLVM profile runtime — it is not an instrumented build" >&2
    echo "coverage: re-run without --no-build" >&2
    exit 2
  fi
fi

# ------------------------------------------------------------ 2. harnesses ----

# The harnesses see the instrumented binary directly (TAURIQT_BIN means "run
# this, never build, never call nix"); everything they need is in this shell.
export TAURIQT_BIN="$APP"
export TAURIQT_EXIT_AFTER_MS="$EXIT_MS"
# A run that ends in SIGKILL (a harness stopping the app's session, say) can
# never flush on its own; the periodic dump is what keeps its coverage, and its
# interval bounds the loss (installPeriodicFlush in qt_runtime.cpp).
export TAURIQT_PROFILE_FLUSH_MS="$PROFILE_FLUSH_MS"
export SMOKE_SKIP_BUILD=1
export SMOKE_TIMEOUT="$SMOKE_T"
export WAYLAND_SMOKE_TIMEOUT="$WAYLAND_T"

# The same software-GL/chromium flags the flake checks use: a headless run has
# no DRM node, and QtWebEngine aborts instead of falling back when GL is absent
# (see docs/nix-qt-gotchas.md). Overridable from the outside.
export LIBGL_ALWAYS_SOFTWARE="${LIBGL_ALWAYS_SOFTWARE:-1}"
export QT_OPENGL="${QT_OPENGL:-software}"
QTWEBENGINE_CHROMIUM_FLAGS="${QTWEBENGINE_CHROMIUM_FLAGS:---no-sandbox --disable-gpu --disable-gpu-compositing}"
if [[ ! -d /dev/shm || ! -w /dev/shm ]]; then
  QTWEBENGINE_CHROMIUM_FLAGS="$QTWEBENGINE_CHROMIUM_FLAGS --disable-dev-shm-usage"
fi
export QTWEBENGINE_CHROMIUM_FLAGS

WAYLAND_OK=0
RAN=0
if [[ "$RUN" == 1 ]]; then
  RAN=1
  # X11 (Xvfb) first: it is the cheap, dependable run. Its verdict is the
  # harness's own banner, not xvfb-run's exit status: some sandboxes make
  # xvfb-run's final `kill $XVFB_PID` fail after the command already exited
  # cleanly, and a real failure never prints the banner anyway.
  rm -f "$RAW_DIR"/smoke-*.profraw
  echo "==> smoke (X11/Xvfb, app exits itself after ${EXIT_MS}ms)"
  smoke_rc=0
  LLVM_PROFILE_FILE="$RAW_DIR/smoke-%p.profraw" QT_QPA_PLATFORM=xcb \
    xvfb-run -a -s "-screen 0 1280x1024x24" bash scripts/smoke.sh \
    >"$LOG_DIR/smoke.log" 2>&1 || smoke_rc=$?
  if ! grep -qF '==> smoke test passed' "$LOG_DIR/smoke.log"; then
    echo "    smoke: FAILED (exit ${smoke_rc}) — tail of $LOG_DIR/smoke.log:" >&2
    tail -n 25 "$LOG_DIR/smoke.log" >&2 || true
    exit 1
  fi
  if ((smoke_rc != 0)); then
    echo "    smoke: ok (xvfb-run cleanup returned ${smoke_rc})"
  else
    echo "    smoke: ok"
  fi

  # Wayland next: it boots its own headless session, so it is only best-effort
  # here (a sandbox without a session bus or a compositor still yields the X11
  # coverage above). COVERAGE_STRICT=1 makes a failure fatal.
  rm -f "$RAW_DIR"/wayland-*.profraw
  echo "==> wayland-smoke (headless sway)"
  if LLVM_PROFILE_FILE="$RAW_DIR/wayland-%p.profraw" \
    bash scripts/wayland-smoke.sh >"$LOG_DIR/wayland.log" 2>&1; then
    WAYLAND_OK=1
    echo "    wayland-smoke: ok"
  elif [[ "${COVERAGE_STRICT:-0}" == 1 ]]; then
    echo "    wayland-smoke: FAILED (COVERAGE_STRICT=1) — tail of $LOG_DIR/wayland.log:" >&2
    tail -n 25 "$LOG_DIR/wayland.log" >&2 || true
    exit 1
  else
    echo "    wayland-smoke: failed to run here (no session/compositor?) — see $LOG_DIR/wayland.log" >&2
    tail -n 5 "$LOG_DIR/wayland.log" >&2 || true
  fi

  # The Qt-free core crate, once it exists, is the one piece `cargo test` can
  # link; its unit tests write their own profiles into the same directory.
  if [[ -d "$ROOT/tauri-runtime-qt-core" ]]; then
    rm -f "$RAW_DIR"/core-*.profraw
    echo "==> unit tests: tauri-runtime-qt-core"
    if ! LLVM_PROFILE_FILE="$RAW_DIR/core-%p.profraw" cargo test -p tauri-runtime-qt-core \
      >"$LOG_DIR/core-tests.log" 2>&1; then
      echo "    core tests: FAILED — tail of $LOG_DIR/core-tests.log:" >&2
      tail -n 25 "$LOG_DIR/core-tests.log" >&2 || true
      exit 1
    fi
    echo "    core tests: ok"
  fi
else
  echo "==> skipping the harness runs (--no-run)"
fi

# The core crate's test executable carries its coverage mapping; llvm-cov needs
# it as an object to attribute those profile records to our sources. Found
# rather than rebuilt, so `--no-run` stays build-free.
CORE_TEST_BIN=""
if [[ -d "$ROOT/tauri-runtime-qt-core" && -d "$ROOT/target/debug/deps" ]]; then
  CORE_TEST_BIN="$(find "$ROOT/target/debug/deps" -maxdepth 1 -type f -name 'tauri-runtime-qt-core-*' \
    -perm -u+x ! -name '*.d' -print 2>/dev/null | head -n1 || true)"
  [[ -n "$CORE_TEST_BIN" ]] || CORE_TEST_BIN=""
fi

# ---------------------------------------------------------------- 3. merge ----

shopt -s nullglob
PROFRAWS=("$RAW_DIR"/*.profraw)
shopt -u nullglob
if ((${#PROFRAWS[@]} == 0)); then
  echo "coverage: no .profraw files in $RAW_DIR — the instrumented app wrote no profile" >&2
  echo "coverage: (did it exit cleanly? TAURIQT_EXIT_AFTER_MS=${EXIT_MS}, log: $LOG_DIR)" >&2
  exit 1
fi
echo "==> merging ${#PROFRAWS[@]} raw profile(s)"
llvm-profdata merge -sparse "${PROFRAWS[@]}" -o "$PROFDATA"

# --------------------------------------------------------------- 4. export ----

# One lcov file for Rust and C++ together; `--ignore-filename-regex` keeps the
# report to *our* sources (cargo registry, rustc's own sources, Qt/Nix store
# headers, generated bridge glue).
OBJECTS=("$APP")
[[ -n "${CORE_TEST_BIN:-}" && -x "${CORE_TEST_BIN:-}" ]] && OBJECTS+=("$CORE_TEST_BIN")
echo "==> exporting lcov from ${#OBJECTS[@]} object(s)"
llvm-cov export \
  --instr-profile="$PROFDATA" \
  --format=lcov \
  --ignore-filename-regex='(/\.cargo/registry/|/rustc/[0-9a-f]+/|/nix/store/|/target/[^/]+/build/)' \
  "${OBJECTS[@]}" >"$LCOV"

# ---------------------------------------------------------------- 5. reports --

FILES_TSV="$COVERAGE_DIR/files.tsv"
TOTALS_TSV="$COVERAGE_DIR/totals.tsv"
rm -f "$FILES_TSV"
awk -v rows="$FILES_TSV" '
  /^SF:/  { file = substr($0, 4); lf = lh = fnf = fnh = brf = brh = 0 }
  /^LF:/  { lf  = substr($0, 4) + 0 }
  /^LH:/  { lh  = substr($0, 4) + 0 }
  /^FNF:/ { fnf = substr($0, 5) + 0 }
  /^FNH:/ { fnh = substr($0, 5) + 0 }
  /^BRF:/ { brf = substr($0, 5) + 0 }
  /^BRH:/ { brh = substr($0, 5) + 0 }
  /^end_of_record/ {
    records++
    TLF += lf; TLH += lh; TBRF += brf; TBRH += brh; TFNF += fnf; TFNH += fnh
    printf "%d\t%d\t%d\t%d\t%d\t%d\t%s\n", lh, lf, brh, brf, fnh, fnf, file >> rows
  }
  END { printf "%d\t%d\t%d\t%d\t%d\t%d\t%d\n", TLH, TLF, TBRH, TBRF, TFNH, TFNF, records }
' "$LCOV" >"$TOTALS_TSV"

read -r TLH TLF TBRH TBRF TFNH TFNF TRECORDS <"$TOTALS_TSV"
if ((TRECORDS == 0 || TLF == 0)); then
  echo "coverage: $LCOV has no coverage records — instrumentation produced no data" >&2
  exit 1
fi

ratio() { awk -v hit="$1" -v found="$2" 'BEGIN { if (found > 0) printf "%.1f%% (%d/%d)", 100 * hit / found, hit, found; else printf "n/a (0/0)" }'; }

echo "==> totals ($TRECORDS file(s))"
printf '    lines     %s\n' "$(ratio "$TLH" "$TLF")"
printf '    functions %s\n' "$(ratio "$TFNH" "$TFNF")"
printf '    branches  %s\n' "$(ratio "$TBRH" "$TBRF")"

echo "==> top files by lines instrumented"
sort -k2,2nr "$FILES_TSV" | head -n 12 |
  while IFS=$'\t' read -r lh lf brh brf fnh fnf file; do
    printf '    lines %-16s branches %-16s %s\n' \
      "$(ratio "$lh" "$lf")" "$(ratio "$brh" "$brf")" "${file#"$ROOT"/}"
  done

# Machine-readable summary for the CI run summary (and for humans on a laptop).
{
  echo "### Coverage — lines $(ratio "$TLH" "$TLF") · functions $(ratio "$TFNH" "$TFNF") · branches $(ratio "$TBRH" "$TBRF")"
  echo
  echo "| file | lines | branches |"
  echo "|---|---|---|"
  sort -k2,2nr "$FILES_TSV" | head -n 10 |
    while IFS=$'\t' read -r lh lf brh brf fnh fnf file; do
      printf '| `%s` | %s | %s |\n' "${file#"$ROOT"/}" "$(ratio "$lh" "$lf")" "$(ratio "$brh" "$brf")"
    done
  echo
  if [[ "$RAN" != 1 ]]; then
    echo "Harnesses: none in this invocation (re-rendered from the existing profiles)."
  elif [[ "$WAYLAND_OK" == 1 ]]; then
    echo "Harnesses: smoke (X11/Xvfb), wayland-smoke (headless sway)."
  else
    echo "Harnesses: smoke (X11/Xvfb); wayland-smoke did not run in this environment."
  fi
} >"$COVERAGE_DIR/summary.md"

HTML_DIR="$COVERAGE_DIR/html"
if command -v genhtml >/dev/null 2>&1; then
  echo "==> html report (genhtml)"
  rm -rf "$HTML_DIR"
  # llvm-cov's lcov is not gcov's: lcov 2.x flags its function records
  # (lambdas/regions it cannot line-attribute) as inconsistent/unsupported, and
  # wraps those in a "corrupt tracefile" error. They are noise for a report we
  # generate ourselves, so ignore exactly those classes.
  if genhtml --branch-coverage --ignore-errors inconsistent,corrupt,unsupported \
    --output-directory "$HTML_DIR" "$LCOV" >"$LOG_DIR/genhtml.log" 2>&1; then
    echo "    $HTML_DIR/index.html ($(wc -c <"$HTML_DIR/index.html") bytes)"
  else
    echo "    genhtml: FAILED — tail of $LOG_DIR/genhtml.log:" >&2
    tail -n 20 "$LOG_DIR/genhtml.log" >&2 || true
    exit 1
  fi
else
  echo "==> genhtml unavailable; skipping coverage/html" >&2
fi

REPORT_DIR="$COVERAGE_DIR/report"
if [[ "${COVERAGE_DOTNET:-1}" == 1 ]] && command -v dotnet >/dev/null 2>&1 &&
  [[ -f "$ROOT/.config/dotnet-tools.json" ]]; then
  echo "==> ReportGenerator (dotnet tool, Html;Badges — the ProjectRead.ing shape)"
  DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1 \
    dotnet tool restore >"$LOG_DIR/dotnet-restore.log" 2>&1 || true
  if DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1 \
    dotnet reportgenerator "-reports:$LCOV" "-targetdir:$REPORT_DIR" \
    "-reporttypes:Html;Badges" >"$LOG_DIR/reportgenerator.log" 2>&1; then
    echo "    $REPORT_DIR/index.html ($(wc -c <"$REPORT_DIR/index.html") bytes)"
  else
    echo "    ReportGenerator unavailable (dotnet tool restore/run failed;" >&2
    echo "    see $LOG_DIR/dotnet-restore.log and $LOG_DIR/reportgenerator.log)" >&2
  fi
elif [[ "${COVERAGE_DOTNET:-1}" == 1 ]]; then
  echo "==> dotnet/ReportGenerator unavailable here; skipping coverage/report" >&2
fi

echo "==> coverage artifacts"
printf '    lcov      %s\n' "$LCOV"
printf '    html      %s/index.html\n' "$HTML_DIR"
[[ -s "$REPORT_DIR/index.html" ]] && printf '    report    %s/index.html\n' "$REPORT_DIR"
printf '    summary   %s/summary.md\n' "$COVERAGE_DIR"
