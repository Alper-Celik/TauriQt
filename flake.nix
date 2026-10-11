# SPDX-FileCopyrightText: 2026 Alper Çelik
# SPDX-License-Identifier: MIT

{
  description = "TauriQt — QtWebEngine sample exploring a Qt-based runtime for Tauri";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # qtwebengine installs into its own prefix; qt-build-utils resolves modules
      # via `qmake -query`, which only knows qtbase's. Merge module prefixes and
      # wrap qmake so module lookups see the merged tree.
      qtMerged =
        pkgs:
        pkgs.symlinkJoin {
          name = "qt-merged";
          paths =
            let q = pkgs.kdePackages;
            in [
              q.qtbase
              q.qtwebengine
              q.qtdeclarative
              q.qtwebchannel
              q.qtpositioning
            ];
        };

      qmakeMerged =
        pkgs:
        pkgs.writeShellScriptBin "qmake" ''
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

      # Everything the Wayland-first test harness needs, all from Nix: headless
      # compositors (sway, cage, weston) with swaybg for backgrounds and grim for
      # captures, dbus for a session bus, Xvfb for the X11 guard, Node for the
      # CDP/WebDriver probes under tools/, chromedriver for the WebDriver path,
      # plus the process/text tools the session scripts use. Nothing is taken
      # from the host, and a build sandbox's PATH is not relied on either, so the
      # core utilities are listed explicitly rather than inherited from stdenv.
      harnessPackages =
        pkgs:
        (with pkgs; [
          sway
          swaybg
          cage
          weston
          grim
          dbus
          xvfb
          xvfb-run
          nodejs
          chromedriver
          coreutils
          gnugrep
          gnused
          gawk
          findutils
          procps
          util-linux
          which
          curl
          jq
        ]);

      # Run a harness script inside a Nix build. The scripts are written for a
      # developer shell: they `cd` to the checkout root, expect the app at
      # ./target/debug/tauriqt and reach the toolchain through `nix develop`. A
      # build has the toolchain on PATH, no `nix` binary and an app that is
      # already built (packages.tauriqt), so the check runs the script unchanged
      # from a writable copy of the checkout with the prebuilt binary linked in
      # and a `nix` shim that unwraps exactly the `develop -c` calls the scripts
      # make; any other invocation is refused loudly rather than skipped.
      harnessCheck =
        pkgs: name: command: extraEnv:
        let
          app = self.packages.${pkgs.stdenv.hostPlatform.system}.tauriqt;
        in
        pkgs.runCommand name
          ({
            nativeBuildInputs = harnessPackages pkgs;

            # How the harness scripts locate the app under test: TAURIQT_BIN is
            # the documented contract (a set TAURIQT_BIN means "run the app
            # directly, never call nix"), APP the convention in the sibling
            # scripts.
            APP = "${app}/bin/tauriqt";
            TAURIQT_BIN = "${app}/bin/tauriqt";

            # Nothing here rebuilds the app: no toolchain, no network. Every
            # script's own skip-build switch is honoured by name.
            SMOKE_SKIP_BUILD = "1";
            WAYLAND_SMOKE_SKIP_BUILD = "1";
            ORACLE_SKIP_BUILD = "1";

            # Bounded runs, so a hung compositor cannot eat the build timeout.
            SMOKE_TIMEOUT = "60";
            WAYLAND_SMOKE_TIMEOUT = "60";

            # Package roots (not binary paths) for the compositor helpers, which
            # is the convention scripts/sway-session.sh resolves as its second
            # choice after PATH. SWAY_BIN/GRIM_BIN are deliberately NOT set here:
            # the two harness scripts disagree on their format (sway-session.sh
            # wants an executable, sway-oracle.sh wants a package root), so only
            # the oracle check below sets them, in its own shape.
            TAURIQT_SWAY_PKG = "${pkgs.sway}";
            TAURIQT_GRIM_PKG = "${pkgs.grim}";

            # QtWebEngine needs a fontconfig database; a build sandbox has none.
            FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };

            # A Nix build sandbox has no /dev/dri and no GL at all, and QtWebEngine
            # aborts (qFatal in SurfaceFactoryQt::GetGLOzone) instead of falling
            # back the way it does when a DRM node merely denies access. Force the
            # software path explicitly; rendering is unaffected for our purposes.
            QTWEBENGINE_CHROMIUM_FLAGS = "--no-sandbox --disable-gpu --disable-gpu-compositing";
            LIBGL_ALWAYS_SOFTWARE = "1";
            QT_OPENGL = "software";
          } // extraEnv)
          ''
            set -eu

            export HOME="$TMPDIR/home"
            export XDG_CONFIG_HOME="$HOME/.config" XDG_CACHE_HOME="$HOME/.cache"
            export XDG_DATA_HOME="$HOME/.local/share"
            export XDG_RUNTIME_DIR="$TMPDIR/run"
            mkdir -p "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME"
            install -d -m 0700 "$XDG_RUNTIME_DIR"

            # The store copy of the checkout is read-only, and the scripts write
            # logs, session state and captures relative to it.
            work="$TMPDIR/work"
            cp -a ${self}/. "$work"
            chmod -R u+w "$work"
            mkdir -p "$work/target/debug" "$work/bin"
            # TAURIQT_BIN/APP is the contract; the target/debug link is the path
            # a script that has not been updated to honour it would fall back to.
            ln -s ${app}/bin/tauriqt "$work/target/debug/tauriqt"
            ln -s ${app}/bin/tauriqt "$work/bin/tauriqt"

            cat > "$work/bin/nix" <<'SHIM'
            #!/bin/sh
            # `nix develop -c <cmd>` only exists to reach the toolchain on a dev
            # machine; here <cmd> already runs in the environment it asks for.
            if [ "''${1:-}" = develop ] && [ "''${2:-}" = -c ]; then
              shift 2
              exec "$@"
            fi
            echo "nix shim: refusing unsupported invocation: nix $*" >&2
            exit 64
            SHIM
            chmod +x "$work/bin/nix"

            # nixpkgs' sway wrapper falls back to `dbus-run-session` when
            # DBUS_SESSION_BUS_ADDRESS is unset, and dbus-daemon's compiled-in
            # default config is the FHS path /etc/dbus-1/session.conf, which a
            # build sandbox does not have (a `dbus-run-session` on PATH cannot
            # help: the wrapper calls it by absolute path). Start a real session
            # bus from the config shipped in the store instead — sway's wrapper
            # reuses it, and Qt clients get a bus like they would on a desktop.
            mkfifo "$TMPDIR/bus.addr"
            dbus-daemon --config-file=${pkgs.dbus}/share/dbus-1/session.conf \
              --nofork --print-address=1 > "$TMPDIR/bus.addr" &
            bus_pid=$!
            trap 'kill "$bus_pid" 2>/dev/null || true' EXIT
            if ! read -r -t 30 DBUS_SESSION_BUS_ADDRESS < "$TMPDIR/bus.addr"; then
              echo "session bus did not come up (dbus-daemon failed?)" >&2
              exit 1
            fi
            export DBUS_SESSION_BUS_ADDRESS
            rm -f "$TMPDIR/bus.addr"

            export PATH="$work/bin:$PATH"
            cd "$work"
            ${command}
            touch "$out"
          '';
    in
    {
      packages = forAllSystems (pkgs: rec {
        tauriqt = pkgs.rustPlatform.buildRustPackage {
          pname = "tauriqt";
          version = "0.1.0";
          src = self;

          cargoLock.lockFile = ./Cargo.lock;

          env.QMAKE = "${qmakeMerged pkgs}/bin/qmake";
          # `cargo test` cannot link this workspace yet: the runtime crate's unit
          # test binary fails with undefined Qt symbols because the `-lQt6*` flags
          # land before our C++ archive and `--as-needed` drops them. The fix is to
          # move the pure logic into a Qt-free `tauri-runtime-qt-core` crate (then
          # unit tests link without Qt); until then behaviour is covered by the
          # harness checks (checks.*), not by cargo test.
          doCheck = false;
          # qtbase's nix setup hook exports QMAKE=<qtbase>/bin/qmake *after* the
          # derivation `env` is applied, and qt-build-utils prefers $QMAKE over
          # PATH. That loses the merged Qt tree (no Qt6WebEngineWidgets.prl) and
          # the build dies in find_prl_for_qt_module. Re-assert the shim once the
          # hooks have run; preBuild runs immediately before buildPhase.
          preBuild = ''
            export QMAKE="${qmakeMerged pkgs}/bin/qmake"
          '';
          nativeBuildInputs = [
            (qmakeMerged pkgs)
            pkgs.cmake
            pkgs.pkg-config
            pkgs.kdePackages.wrapQtAppsHook
          ];
          buildInputs = [
            pkgs.kdePackages.qtbase
            pkgs.kdePackages.qtwebengine
            pkgs.libGL
            # tauri-runtime links the GTK/webkit stack unconditionally on Linux;
            # these must be buildInputs (not nativeBuildInputs) so their .pc files
            # land on PKG_CONFIG_PATH for the -sys crates (atk, gdk-pixbuf, …).
            pkgs.glib
            pkgs.gtk3
            pkgs.webkitgtk_4_1
            pkgs.atk
          ];

          meta = with pkgs.lib; {
            description = "Sample QtWebEngine window driven from Rust via cxx-qt";
            license = licenses.mit;
            mainProgram = "tauriqt";
          };
        };
        default = tauriqt;
      });

      apps = forAllSystems (pkgs: {
        default = {
          type = "app";
          program = "${self.packages.${pkgs.system}.tauriqt}/bin/tauriqt";
        };
      });

      devShells = forAllSystems (pkgs: rec {
        default = pkgs.mkShell {
          QMAKE = "${qmakeMerged pkgs}/bin/qmake";
          packages = [
            (qmakeMerged pkgs)
            pkgs.cargo
            pkgs.rustc
            pkgs.rustfmt
            pkgs.clippy
            pkgs.cmake
            pkgs.reuse
            pkgs.just
            pkgs.pkg-config
            pkgs.glib
            pkgs.gtk3
            pkgs.webkitgtk_4_1
            (qtMerged pkgs)
          ];
        };

        # Harness shell: everything the Wayland/X11 test harness drives, plus the
        # default shell's cargo/qmake/merged-Qt toolchain (via inputsFrom). All of
        # it is Nix-provided; a CI runner contributes nothing but Nix itself.
        ci = pkgs.mkShell {
          QMAKE = "${qmakeMerged pkgs}/bin/qmake";
          inputsFrom = [ default ];
          packages = [ pkgs.clang ] ++ harnessPackages pkgs;
        };
      });

      checks = forAllSystems (pkgs: {
        # X11 guard: the engine-level smoke (scripts/smoke.sh) on a virtual X
        # server. Small and independent of the Wayland harness.
        smoke = harnessCheck pkgs "tauriqt-smoke" ''
          xvfb-run -a -s "-screen 0 1280x1024x24" bash scripts/smoke.sh
        '' {
          QT_QPA_PLATFORM = "xcb";
        };

        # Wayland harness: scripts/wayland-smoke.sh (which drives a headless sway
        # session through scripts/sway-session.sh) runs inside the build; every
        # compositor and tool it needs is a Nix dependency of this check.
        wayland-smoke = harnessCheck pkgs "tauriqt-wayland-smoke" ''
          bash scripts/wayland-smoke.sh
        '' { };

        # scripts/sway-oracle.sh: the same headless-sway exercise from the other
        # direction (the script starts and drives its own session). Unlike
        # sway-session.sh it wants SWAY_BIN/GRIM_BIN as package roots and, with
        # neither set, would fall back to `nix build nixpkgs#sway` — impossible
        # inside a build, hence both are pinned here.
        sway-oracle = harnessCheck pkgs "tauriqt-sway-oracle" ''
          bash scripts/sway-oracle.sh
        '' {
          SWAY_BIN = "${pkgs.sway}";
          GRIM_BIN = "${pkgs.grim}";
        };
      });
    };
}
