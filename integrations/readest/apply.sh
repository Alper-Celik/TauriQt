#!/usr/bin/env bash
# Put Readest on the Qt runtime: clone it at the pinned commit, apply the patch
# kept next to this script, and point the patched manifest at this repo's
# `tauri-runtime-qt`.
#
# We keep a patch, not a fork: the checkout below is disposable and gitignored.
set -euo pipefail

READEST_REPO="${READEST_REPO:-https://github.com/readest/readest}"
READEST_SHA="${READEST_SHA:-385a322b8c1934375d02c50435b07c268d8ed3de}"

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
runtime="$(cd "$here/../.." && pwd)/tauri-runtime-qt"
# Reuse an existing checkout with READEST_CHECKOUT (handy when its node deps
# are already installed); the default is disposable and gitignored.
checkout="${READEST_CHECKOUT:-$here/checkout}"

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

# Readest keeps the tauri it builds against, and some path dependencies, in
# submodules. Only this set is needed here: `packages/tauri` is what the root
# [patch.crates-io] resolves `tauri` to, the two plugins are path dependencies
# of src-tauri, and `foliate-js` is a pnpm workspace package of the frontend.
# (Not listed: qcms/js-mdict/simplecc-wasm — not workspace members, and install
# resolves without them — nor .claude/skills/gstack, nor OpenCC, which only
# `--recursive` would drag in.)
git -C "$checkout" submodule update --init \
  packages/tauri \
  packages/foliate-js \
  apps/readest-app/src-tauri/plugins/tauri-plugin-turso \
  apps/readest-app/src-tauri/plugins/tauri-plugin-webview-upgrade

echo "readest $READEST_SHA patched; qt runtime -> $runtime"
echo "next: integrations/readest/run.sh (from this repo's nix dev shell)"
