#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Cut a release: bump both runtime crates' version, commit that, and tag the
# commit `v<version>`.
#
#   scripts/release.sh            # patch: 0.0.0 -> 0.0.1, then 0.0.2, 0.0.3 …
#   scripts/release.sh minor      # 0.0.5 -> 0.1.0 (a breaking change under 0.0.x)
#   scripts/release.sh major      # 1.0.0
#   scripts/release.sh 0.2.0      # exactly that version
#   scripts/release.sh --push     # push main and the tag, which is what releases
#
# The tag is the release: `.github/workflows/publish.yml` fires on `v*`, refuses a
# tag that disagrees with the manifests' version, waits for CI on that commit and
# then publishes. Making the tag here — on the bump commit, from a clean tree —
# is what keeps those two checks satisfiable; nothing else should create them.
#
# Patch is the default because the crates are 0.0.x: `0.0.0 -> 0.0.1` is the only
# bump a `tauri-runtime-qt = "0.0"` requirement keeps resolving, so a consumer
# follows the releases without editing their manifest. Under 0.0.x a *minor* bump
# is a breaking change by semver and needs that edit, which is why it is explicit.
#
# `--push` requires `main`: CI (and so the publish gate) runs on pushes to main,
# and a release from a branch nobody grades would sit at the gate for 30 minutes
# and then refuse itself.
set -euo pipefail

cd "$(dirname "$0")/.."

core_manifest=tauri-runtime-qt-core/Cargo.toml
runtime_manifest=tauri-runtime-qt/Cargo.toml

bump=patch
wanted=
push=0
for arg in "$@"; do
  case "$arg" in
    patch|minor|major) bump="$arg" ;;
    --push) push=1 ;;
    # An explicit version is validated before anything is written: the seds would
    # happily put a typo in the manifests, and cargo would only object afterwards,
    # leaving a half-edited tree behind.
    *)
      if [[ "$arg" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        wanted="$arg"
      else
        echo "usage: release.sh [patch|minor|major|<X.Y.Z>] [--push]" >&2
        exit 2
      fi
      ;;
  esac
done

command -v cargo >/dev/null || { echo "no cargo: run this from the dev shell (just release does)" >&2; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo "working tree is dirty; commit first" >&2; exit 1; }
if [[ "$push" == 1 ]]; then
  branch="$(git rev-parse --abbrev-ref HEAD)"
  [[ "$branch" == main ]] || { echo "--push from '$branch': releases are cut from main (that is where CI runs)" >&2; exit 1; }
fi

current="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$runtime_manifest" | head -1)"
version="$wanted"
if [[ -z "$version" ]]; then
  IFS=. read -r major minor patch <<<"$current"
  case "$bump" in
    patch) patch=$((patch + 1)) ;;
    minor) minor=$((minor + 1)); patch=0 ;;
    major) major=$((major + 1)); minor=0; patch=0 ;;
  esac
  version="$major.$minor.$patch"
fi

if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  echo "tag v$version already exists" >&2
  exit 1
fi

# A release cannot go backwards — the registry would take the version (it is new
# to crates.io) while every consumer still resolved to the newer one, and the tag
# series would read 0.5.0, 0.0.1, 0.0.2. Only an explicit version can get here.
IFS=. read -r c_major c_minor c_patch <<<"$current"
IFS=. read -r n_major n_minor n_patch <<<"$version"
if (( n_major < c_major || (n_major == c_major && n_minor < c_minor) \
     || (n_major == c_major && n_minor == c_minor && n_patch < c_patch) )); then
  echo "$version is lower than the manifests' $current" >&2
  exit 1
fi

if [[ "$version" == "$current" ]]; then
  echo "== $version is the manifests' version already: tagging it as it stands =="
else
  echo "== $current -> $version =="
  sed -i "s|^version = \"[^\"]*\"|version = \"$version\"|" "$core_manifest" "$runtime_manifest"
  sed -i "s|^tauri-runtime-qt-core = .*|tauri-runtime-qt-core = { path = \"../tauri-runtime-qt-core\", version = \"$version\" }|" "$runtime_manifest"
  # The lockfile carries workspace members' versions too, and a release commit
  # that leaves it stale is a dirty tree for the next one.
  cargo metadata --format-version 1 >/dev/null
  git add "$core_manifest" "$runtime_manifest" Cargo.lock
  git commit -q -m "release: $version"
fi

# Read back rather than trust the two seds: a version skew here is exactly what
# publish.sh refuses, and it would be found three steps later, mid-release.
core="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$core_manifest" | head -1)"
runtime="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$runtime_manifest" | head -1)"
dep="$(sed -n 's/^tauri-runtime-qt-core = .*version = "\([^"]*\)".*/\1/p' "$runtime_manifest")"
if [[ "$core" != "$version" || "$runtime" != "$version" || "$dep" != "$version" ]]; then
  echo "version skew after the bump: core=$core runtime=$runtime dep=$dep, wanted $version" >&2
  exit 1
fi

git tag -a "v$version" -m "tauri-runtime-qt{,-core} $version"
echo "== tagged v$version at $(git rev-parse --short HEAD) =="

if [[ "$push" == 1 ]]; then
  # Main first, then the tag: the tag starts the publish workflow, which waits for
  # this push's CI run.
  git push origin HEAD
  git push origin "v$version"
  echo "== pushed: .github/workflows/publish.yml is releasing $version =="
else
  echo "publish it with:"
  echo "  git push origin HEAD && git push origin v$version"
fi
