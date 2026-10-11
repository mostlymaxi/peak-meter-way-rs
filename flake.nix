{
  description = "peak-meter-way: low-overhead PipeWire peak meter for waybar";

  # Only nixpkgs, so consumers can `inputs.nixpkgs.follows = "nixpkgs"`.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      overlays.default = final: _prev: {
        peak-meter-way = final.callPackage ./package.nix { };
      };

      packages = forAllSystems (pkgs: rec {
        peak-meter-way = pkgs.callPackage ./package.nix { };
        default = peak-meter-way;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.peak-meter-way ];
          packages = with pkgs; [
            rustfmt
            clippy
            pre-commit
          ];
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
