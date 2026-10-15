#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Publish the runtime crates to crates.io, in dependency order:
#
#   scripts/publish.sh --dry-run   # package + verify-build both, upload nothing
#   scripts/publish.sh             # upload (needs CARGO_REGISTRY_TOKEN / `cargo login`)
#
# Only `tauri-runtime-qt` and `tauri-runtime-qt-core` publish; the `tauriqt` demo
# is `publish = false` — it is this repo's test app, and the harnesses grade it.
#
# The order is not optional. `cargo publish` builds the *packaged* crate, where
# the path dependency on the core crate has been rewritten to `version = "x.y.z"`,
# so core has to be on crates.io — and visible in the index cargo reads — first.
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

# Uploading is irreversible, so uploading something that is not in git would be
# worse than an error here. cargo refuses a dirty tree too, but only after the
# registry round-trip.
[[ -z "$(git status --porcelain)" ]] || { echo "working tree is dirty; commit first" >&2; exit 1; }

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' tauri-runtime-qt/Cargo.toml | head -1)"

if [[ "$dry_run" == 1 ]]; then
  echo "== dry run: tauri-runtime-qt $version =="
  # `--workspace --exclude tauriqt` is the only mode that packages *and*
  # verify-builds both published crates before the first release: standalone
  # (`cargo package -p tauri-runtime-qt`) resolves the runtime's core dependency
  # from crates.io, where it does not exist yet, while the workspace form builds
  # it from a temporary local registry. The demo is excluded because its
  # `frontendDist` points outside its package directory (`../dist`), which is
  # also why it is `publish = false`.
  cargo package --workspace --exclude tauriqt
  echo "== both crates packaged and verify-built, nothing uploaded =="
  exit 0
fi

echo "== publishing tauri-runtime-qt-core $version =="
cargo publish -p tauri-runtime-qt-core

# The index this waits on is the sparse one cargo itself resolves against, not the
# web API: a version can be in the database and still not resolvable here.
index="https://index.crates.io/ta/ur/tauri-runtime-qt-core"
echo "== waiting for tauri-runtime-qt-core $version in the index =="
for _ in $(seq 1 60); do
  if curl -sf "$index" | grep -q "\"vers\":\"$version\""; then
    break
  fi
  sleep 5
done
if ! curl -sf "$index" | grep -q "\"vers\":\"$version\""; then
  echo "core is still not in the index after 5 min; once it is, finish with:" >&2
  echo "  cargo publish -p tauri-runtime-qt" >&2
  exit 1
fi

cargo publish -p tauri-runtime-qt
echo "== published: https://crates.io/crates/tauri-runtime-qt/$version =="
