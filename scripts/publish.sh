#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Publish the runtime crates to crates.io, in dependency order:
#
#   scripts/publish.sh --dry-run   # package + verify-build both, upload nothing
#   scripts/publish.sh             # upload (CARGO_REGISTRY_TOKEN, or `cargo login`)
#
# `.github/workflows/publish.yml` calls exactly these two modes on a version tag,
# so this script is the whole release procedure either way; both are hand-runnable
# from the dev shell (`just publish-dry`, `just publish`).
#
# Only `tauri-runtime-qt` and `tauri-runtime-qt-core` publish; the `tauriqt` demo
# is `publish = false` — it is this repo's test app, and the harnesses grade it.
#
# Three things this script refuses to do:
#
#   * upload from a dirty tree (an upload is irreversible, and CI would be
#     publishing something that is not in git);
#   * publish manifests whose versions disagree — the runtime's dependency on the
#     core crate names a version, and a tag is supposed to name all three;
#   * package the runtime half before the core half is in the index cargo reads.
#     `cargo publish` builds the *packaged* crate, where the path dependency has
#     been rewritten to `version = "x.y.z"`, so core has to be resolvable from
#     crates.io first — hence the publish order, and the wait for the index.
#
# Versions already on crates.io are skipped rather than failed: a release that
# half-succeeded (core up, runtime refused) is finished by re-running this.
#
# The runtime's build script resolves Qt through qmake (Widgets + WebEngineWidgets),
# so packaging it needs the dev shell, like every other recipe here.
set -euo pipefail

cd "$(dirname "$0")/.."

dry_run=0
case "${1:-}" in
  --dry-run) dry_run=1 ;;
  "") ;;
  *) echo "usage: publish.sh [--dry-run]" >&2; exit 2 ;;
esac

export QMAKE="${QMAKE:-$(command -v qmake)}"

manifest_version() {
  sed -n 's/^version = "\([^"]*\)"/\1/p' "$1" | head -1
}

# The sparse index cargo itself resolves against, not the web API: a version can
# be in the database and still not be resolvable from here.
index_url() {
  local name="$1"
  case ${#name} in
    1) echo "https://index.crates.io/1/$name" ;;
    2) echo "https://index.crates.io/2/$name" ;;
    3) echo "https://index.crates.io/3/${name:0:1}/$name" ;;
    *) echo "https://index.crates.io/${name:0:2}/${name:2:2}/$name" ;;
  esac
}

published() {
  curl -sf "$(index_url "$1")" | grep -q "\"vers\":\"$2\""
}

core_version="$(manifest_version tauri-runtime-qt-core/Cargo.toml)"
runtime_version="$(manifest_version tauri-runtime-qt/Cargo.toml)"
dep_version="$(sed -n 's/^tauri-runtime-qt-core = .*version = "\([^"]*\)".*/\1/p' tauri-runtime-qt/Cargo.toml)"
version="$runtime_version"

if [[ "$core_version" != "$runtime_version" || "$dep_version" != "$core_version" ]]; then
  echo "version skew: tauri-runtime-qt-core=$core_version tauri-runtime-qt=$runtime_version, which requires core=$dep_version" >&2
  exit 1
fi

if [[ "$dry_run" == 1 ]]; then
  echo "== dry run: tauri-runtime-qt{,-core} $version =="
  # `--workspace --exclude tauriqt` is the only mode that packages *and*
  # verify-builds both published crates before the first release: standalone
  # (`cargo package -p tauri-runtime-qt`) resolves the runtime's core dependency
  # from crates.io, where it may not exist yet, while the workspace form builds
  # it from a temporary local registry. The demo is excluded because its
  # `frontendDist` points outside its package directory (`../dist`), which is
  # also why it is `publish = false`.
  cargo package --workspace --exclude tauriqt
  echo "== both crates packaged and verify-built, nothing uploaded =="
  exit 0
fi

# Uploading is irreversible, so uploading something that is not in git would be
# worse than an error here. cargo refuses a dirty tree too, but only after the
# registry round-trip.
[[ -z "$(git status --porcelain)" ]] || { echo "working tree is dirty; commit first" >&2; exit 1; }

if [[ -z "${CARGO_REGISTRY_TOKEN:-}" && ! -f "${CARGO_HOME:-$HOME/.cargo}/credentials.toml" ]]; then
  echo "no crates.io token: export CARGO_REGISTRY_TOKEN or run cargo login (CI mints one via trusted publishing)" >&2
  exit 1
fi

# Core first, then wait for it to be resolvable, then the half that depends on it.
if published tauri-runtime-qt-core "$version"; then
  echo "== tauri-runtime-qt-core $version is already on crates.io =="
else
  echo "== publishing tauri-runtime-qt-core $version =="
  cargo publish -p tauri-runtime-qt-core

  echo "== waiting for tauri-runtime-qt-core $version in the index =="
  for _ in $(seq 1 60); do
    if published tauri-runtime-qt-core "$version"; then
      break
    fi
    sleep 5
  done
  if ! published tauri-runtime-qt-core "$version"; then
    echo "core is still not in the index after 5 min; re-run this once it is:" >&2
    echo "  scripts/publish.sh" >&2
    exit 1
  fi
fi

if published tauri-runtime-qt "$version"; then
  echo "== tauri-runtime-qt $version is already on crates.io =="
else
  echo "== publishing tauri-runtime-qt $version =="
  cargo publish -p tauri-runtime-qt
fi

echo "== crates.io: https://crates.io/crates/tauri-runtime-qt/$version =="
