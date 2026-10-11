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
    in
    {
      packages = forAllSystems (pkgs: rec {
        tauriqt = pkgs.rustPlatform.buildRustPackage {
          pname = "tauriqt";
          version = "0.1.0";
          src = self;

          cargoLock.lockFile = ./Cargo.lock;

          env.QMAKE = "${qmakeMerged pkgs}/bin/qmake";
          nativeBuildInputs = [
            (qmakeMerged pkgs)
            pkgs.cmake
            pkgs.kdePackages.wrapQtAppsHook
          ];
          buildInputs = [
            pkgs.kdePackages.qtbase
            pkgs.kdePackages.qtwebengine
            pkgs.libGL
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

      devShells = forAllSystems (pkgs: {
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
            pkgs.libGL
            (qtMerged pkgs)
          ];
        };
      });
    };
}
