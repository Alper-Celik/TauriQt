# Nix ⇄ Qt ⇄ sandbox gotchas (all measured in this repo)

Everything here was hit for real while building `tauri-runtime-qt`; each item cost at least one
debugging cycle. Read this before touching `flake.nix`, `build.rs`, or the CI checks.

## 1. Qt modules live in separate Nix prefixes — `qmake -query` only knows one

`qmake -query QT_INSTALL_LIBS` reports qtbase's prefix. `qtwebengine` (and `qtdeclarative`,
`qtwebchannel`, `qtpositioning`) install into **their own** store paths, so anything that resolves Qt
modules through qmake — which is exactly what `qt-build-utils` (and therefore `cxx-qt-build`) does —
cannot see `Qt6WebEngineWidgets.prl`:

```
thread 'main' panicked at qt-build-utils-0.10.0/src/installation/shared.rs:181:
Could not find a prl path for Qt module: WebEngineWidgets
```

**Fix (in `flake.nix`):** a merged tree plus a qmake shim.

```nix
qtMerged = pkgs: pkgs.symlinkJoin {
  name = "qt-merged";
  paths = with pkgs.kdePackages; [ qtbase qtwebengine qtdeclarative qtwebchannel qtpositioning ];
};

qmakeMerged = pkgs: pkgs.writeShellScriptBin "qmake" ''
  real="${pkgs.kdePackages.qtbase}/bin/qmake"
  base="${pkgs.kdePackages.qtbase}"
  merged="${qtMerged pkgs}"
  if [ "''${1:-}" = "-query" ]; then
    v="$("$real" -query "''${2:-}")"
    echo "''${v//$base/$merged}"
  else
    exec "$real" "$@"
  fi
'';
```

## 2. `QMAKE` must be pinned — and re-pinned *after* the setup hooks

Two independent traps, both silent:

- `qt-build-utils` looks for **`qmake6` before `qmake`** on PATH. A distro/Qt `qmake6` wins over our
  shim, and the build fails with the same `prl` panic. So `QMAKE` must be set explicitly rather than
  relied on through PATH ordering.
- In a `mkDerivation`, **`env.QMAKE = …` is not enough**: nixpkgs' qtbase setup hook exports
  `QMAKE=<qtbase>/bin/qmake` *after* the derivation env is applied, and `qt-build-utils` prefers
  `$QMAKE` over PATH. Measured inside the build:

  ```
  QMAKE env        : /nix/store/…-qtbase-6.11.2/bin/qmake     ← what the build actually used
  which qmake      : /nix/store/…-qmake/bin/qmake             ← our shim, correct but ignored
  qmake -query LIBS: /nix/store/…-qt-merged/lib               ← shim reports the merged path fine
  ```

  **Fix:** re-assert it after the hooks, e.g. `preBuild = ''export QMAKE="${qmakeMerged pkgs}/bin/qmake"'';`
  (keep `env.QMAKE` too — other tools read it). The devshell has no such hook, which is why the same
  expression works interactively and fails under `nix build`.

## 3. GTK/webkit `.pc` files must come from `buildInputs`, and `atk` is not implied

`tauri-runtime` links the GTK stack unconditionally on Linux, so `-sys` crates run `pkg-config`.
Putting `glib`/`gtk3`/`webkitgtk_4_1` in **`nativeBuildInputs`** does not put their `.pc` files on the
target `PKG_CONFIG_PATH`:

```
error: failed to run custom build command for `atk-sys v0.18.2`
The system library `atk` required by crate `atk-sys` was not found
```

**Fix:** those belong in `buildInputs`, and **`atk` must be listed explicitly** (it is not reachable
just by depending on `gtk3`).

## 4. The agent sandbox breaks `nix` unless `LD_LIBRARY_PATH` is cleared

Inside the agent's firejail, the inherited `LD_LIBRARY_PATH` (gcc's libstdc++) makes the `lix` binary
fail with `GLIBCXX_3.4.36 not found`. Every nix command must be run as
`LD_LIBRARY_PATH= nix …`. Harmless elsewhere; confusing the first time.

## 5. Chromium's sandbox is unavailable inside the firejail

`QTWEBENGINE_CHROMIUM_FLAGS=--no-sandbox` is required inside the agent sandbox and in containers
(no setuid sandbox, restricted namespaces). It is *not* required on a normal desktop session, where
we prefer to leave the sandbox on. When `/dev/shm` is missing or unwritable (a Nix **build** sandbox),
`--disable-dev-shm-usage` is added automatically; the effective flag set is printed by the harnesses.

## 6. Headless Wayland *does* work inside a Nix build sandbox — with one prerequisite

Verified by building (not on the host): `sway` with `WLR_BACKENDS=headless WLR_RENDERER=pixman` starts
inside a `nix build`, creates its socket, and a real client paints (`grim` produced a 2759-byte PNG).
The prerequisite is **D-Bus**:

- nixpkgs' sway wrapper falls back to an **absolute** `dbus-run-session` when
  `DBUS_SESSION_BUS_ADDRESS` is unset, and dbus-daemon's compiled-in config is the FHS path
  (`/etc/dbus-1/session.conf`) which does not exist in the build sandbox → sway exits **127**;
- a PATH shim for `dbus-run-session` does **not** help (the wrapper uses the absolute path);
- **fix:** start a session bus from `${dbus}/share/dbus-1/session.conf` and export
  `DBUS_SESSION_BUS_ADDRESS` before launching sway (what `checks.wayland-smoke` does).

## 7. QtWebEngine under a headless compositor needs no extra flags — but a *build sandbox* does

Measured on a normal session and in the agent sandbox (where `/dev/dri` exists but denies access):
QtWebEngine 6.11.2 renders and executes JS under `WLR_BACKENDS=headless WLR_RENDERER=pixman` with
**only** `--no-sandbox` — no `--disable-gpu`, no ozone switch. Harmless noise you can ignore:
`MESA: error: Opening /dev/dri/card2 failed`, one `ContextResult::kTransientFailure:
Failed to send GpuControl.CreateCommandBuffer.`, and a Chromium system-bus denial.

Inside a **Nix build sandbox** the situation is different: there is no `/dev/dri` node *at all*, and
QtWebEngine does not fall back — it aborts:

```
#7  qAbort()
#8  qt_message()
#9  QMessageLogger::fatal()
#10 QtWebEngineCore::SurfaceFactoryQt::GetGLOzone()
#11 gl::init::CreateGLContext()
```

The app dies before emitting a single trace line, so a harness reports "every expectation failed"
(which is exactly how this was found). Fix, applied in the check environment
(`harnessCheck` in `flake.nix`):

```nix
QTWEBENGINE_CHROMIUM_FLAGS = "--no-sandbox --disable-gpu --disable-gpu-compositing";
LIBGL_ALWAYS_SOFTWARE = "1";
QT_OPENGL = "software";
```

and the checks also provide `FONTCONFIG_FILE` (a build sandbox has no font database, and
QtWebEngine wants one) plus writable `HOME`/`XDG_*`.

## 8. `#!/usr/bin/env` does not exist inside a Nix build

A script that executes a sibling *by path* dies with `bad interpreter: No such file or directory`,
because the sandbox has no `/usr/bin/env`. Keep the ergonomic shebang for humans, but invoke
siblings through the running interpreter:

```bash
BASH_BIN="${BASH:-bash}"
"$BASH_BIN" "$SELF_DIR/other-script.sh" start
```

Related: a failed command substitution under `set -u` can hide the real error behind
`unbound variable`; read such variables with `"${VAR:-}"` and fail with a clear message.
