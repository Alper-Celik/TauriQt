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
    in
    {
      packages = forAllSystems (pkgs: rec {
        tauriqt = pkgs.stdenv.mkDerivation {
          pname = "tauriqt";
          version = "0.1.0";
          src = self;
          nativeBuildInputs = [
            pkgs.cmake
            pkgs.kdePackages.wrapQtAppsHook
          ];
          buildInputs = [
            pkgs.kdePackages.qtbase
            pkgs.kdePackages.qtwebengine
          ];
          meta = with pkgs.lib; {
            description = "Sample QtWebEngine browser window";
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
          packages = [
            pkgs.cmake
            pkgs.ninja
            pkgs.reuse
            pkgs.kdePackages.qtbase
            pkgs.kdePackages.qtwebengine
            pkgs.kdePackages.wrapQtAppsHook
          ];
        };
      });
    };
}
