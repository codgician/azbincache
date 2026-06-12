{
  description = "azbincache — publish a Nix binary cache to Azure Blob Storage";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs { inherit system; };

        # Runtime tools azbincache shells out to.
        runtimeInputs = [ pkgs.nix ];

        azbincache = pkgs.rustPlatform.buildRustPackage {
          pname = "azbincache";
          version = "0.1.0";
          src = pkgs.lib.cleanSource ./.;
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [
            pkgs.openssl
            pkgs.xz
          ];

          # Run only the in-crate unit tests during the sandboxed build; the
          # Azurite/NixOS integration tests need services and run via the
          # devshell and `nix flake check` respectively.
          cargoTestFlags = [ "--lib" ];
          nativeCheckInputs = runtimeInputs;

          meta = with pkgs.lib; {
            description = "Publish a Nix binary cache to Azure Blob Storage with upstream-aware dedup and commit-windowed GC";
            license = licenses.mit;
            mainProgram = "azbincache";
          };
        };
      in
      {
        packages = {
          default = azbincache;
          azbincache = azbincache;
        };

        apps.default = {
          type = "app";
          program = "${azbincache}/bin/azbincache";
        };

        checks = {
          inherit azbincache;

          clippy = azbincache.overrideAttrs (old: {
            pname = "azbincache-clippy";
            nativeBuildInputs = (old.nativeBuildInputs or [ ]) ++ [ pkgs.clippy ];
            buildPhase = "cargo clippy --all-targets -- --deny warnings";
            installPhase = "touch $out";
            doCheck = false;
          });

          fmt =
            pkgs.runCommand "azbincache-fmt"
              {
                nativeBuildInputs = [
                  pkgs.cargo
                  pkgs.rustfmt
                ];
              }
              ''
                cd ${pkgs.lib.cleanSource ./.}
                cargo fmt --check
                touch $out
              '';

          end-to-end = pkgs.testers.runNixOSTest (
            import ./tests/nixos/end-to-end.nix {
              azbincachePackage = azbincache;
            }
          );
        };

        devShells.default = pkgs.mkShell {
          packages = [
            pkgs.rustc
            pkgs.cargo
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.pkg-config
            pkgs.openssl
            pkgs.xz
            pkgs.nix # for integration tests / golden fixtures
            pkgs.azurite # local Azure Blob emulator for integration tests
          ];

          RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";
        };

        formatter = pkgs.nixfmt-rfc-style;
      }
    );
}
