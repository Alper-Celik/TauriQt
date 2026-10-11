#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT
# Put Readest on the Qt runtime: clone it at the pinned commit, apply the patch
# kept next to this script, and point the patched manifest at this repo's
# `tauri-runtime-qt`.
#
# We keep a patch, not a fork: the checkout below is disposable and gitignored.
set -euo pipefail

READEST_REPO="${READEST_REPO:-https://github.com/readest/readest}"
READEST_SHA="${READEST_SHA:-385a322b8c1934375d02c50435b07c268d8ed3de}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
runtime="$repo/tauri-runtime-qt"
# Readest can be told where to land with READEST_CHECKOUT. The default is a
# sibling of this repo, deliberately *outside* it: this repo is a cargo
# workspace, and a checkout nested inside one makes cargo read Readest's
# vendored tauri fork against our workspace root instead of its own (it then
# fails with "workspace.package.edition was not defined"). The checkout also
# needs several GB (node_modules + a Rust target dir), so it wants a roomy
# filesystem.
checkout="${READEST_CHECKOUT:-$(dirname "$repo")/readest-qt-checkout}"
case "$(cd "$(dirname "$checkout")" 2>/dev/null && pwd)/" in
  "$repo"/*)
    echo "READEST_CHECKOUT must live outside this repo: $checkout is inside $repo" >&2
    echo "(a checkout nested in a cargo workspace makes cargo resolve the wrong workspace root)" >&2
    exit 1
    ;;
esac

[[ -d "$runtime" ]] || { echo "no tauri-runtime-qt next to $here" >&2; exit 1; }

if [[ ! -d "$checkout/.git" ]]; then
  echo "cloning $READEST_REPO"
  git clone --filter=blob:none "$READEST_REPO" "$checkout"
fi

git -C "$checkout" fetch --quiet origin "$READEST_SHA" 2>/dev/null ||
  git -C "$checkout" fetch --quiet origin
git -C "$checkout" checkout --quiet --force "$READEST_SHA"
git -C "$checkout" clean -qfd

git -C "$checkout" apply "$here/readest-qt-runtime.patch"
sed -i "s|@TAURIQT_RUNTIME@|$runtime|" "$checkout/apps/readest-app/src-tauri/Cargo.toml"

# Readest keeps the tauri it builds against, and several path dependencies, in
# submodules; take them all. It is someone else's repo and this checkout is
# disposable, so there is nothing to be careful about here.
git -C "$checkout" submodule update --init --recursive

echo "readest $READEST_SHA patched; qt runtime -> $runtime"
echo "next: integrations/readest/run.sh (from this repo's nix dev shell)"
