{
  description = "arches, a bridge between Arc Timeline Recorder backups and other tools.";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rust-analyzer
            clippy
            rustfmt
            nodejs_26
            pnpm
            sqlite
          ];
        };
      });

      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "arches";
          version = (pkgs.lib.importTOML ./Cargo.toml).package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
          # The GPX ingest turns coordinates into local offsets through jiff, which reads the
          # system tzdb. The build sandbox has none, and without this every offset in the test
          # suite would quietly come out as UTC.
          TZDIR = "${pkgs.tzdata}/share/zoneinfo";
        };
      });
    };
}
