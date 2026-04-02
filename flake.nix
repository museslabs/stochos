{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs = {
    self,
    nixpkgs,
    flake-utils,
    crane,
    ...
  }:
    (flake-utils.lib.eachDefaultSystem (
      system: let
        pkgs = import nixpkgs {
          inherit system;
        };

        craneLib = crane.mkLib pkgs;
        stochos = pkgs.callPackage ./nix/package.nix {inherit craneLib;};
      in {
        formatter = pkgs.alejandra;
        checks = {inherit stochos;};

        packages = rec {
          inherit stochos;
          default = stochos;
        };

        apps = rec {
          default = flake-utils.lib.mkApp {
            drv = stochos;
          };
          stochos = default;
        };

        devShells.default = craneLib.devShell {
          checks = self.checks.${system};
          inputsFrom = [stochos];

          packages = with pkgs; [
            rust-analyzer
          ];

          shellHook = ''
            export LD_LIBRARY_PATH=${stochos.passthru.runtimeLibsPath}:$LD_LIBRARY_PATH
          '';
        };
      }
    ))
    // {
      homeModules = rec {
        default = import ./nix/module.nix self.packages;
        stochos = default;
      };
    };
}
