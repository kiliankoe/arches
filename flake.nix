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
      # Pinned to a major: pnpm's store format changes between majors, which would turn a
      # nixpkgs bump into a hash mismatch in fetchPnpmDeps. The dev shell uses the same one so
      # the lockfile it writes is the one the package reads.
      pnpmFor = pkgs: pkgs.pnpm_11;
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
            (pnpmFor pkgs)
            sqlite
          ];
        };
      });

      packages = forAllSystems (
        pkgs:
        let
          pname = "arches";
          version = (pkgs.lib.importTOML ./Cargo.toml).package.version;
          pnpm = pnpmFor pkgs;
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            inherit pname version;
            src = self;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [
              pkgs.nodejs_26
              pnpm
              pkgs.pnpmConfigHook
            ];
            pnpmDeps = pkgs.fetchPnpmDeps {
              inherit pname version pnpm;
              src = self;
              sourceRoot = "source/web";
              fetcherVersion = 4;
              hash = "sha256-ZppvpZ7tHKbsYbQGsAiM9LP7sCVjHE/7WcoufTKH0Gw=";
            };
            pnpmRoot = "web";
            # build.rs embeds whatever is in web/dist when cargo runs, so the UI goes first.
            preBuild = "pnpm --dir web build";

            # The GPX ingest turns coordinates into local offsets through jiff, which reads the
            # system tzdb. The build sandbox has none, and without this every offset in the test
            # suite would quietly come out as UTC.
            TZDIR = "${pkgs.tzdata}/share/zoneinfo";
          };
        }
      );
    };
}
