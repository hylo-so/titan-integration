{
  description = "Titan router integration for Hylo V2";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs";
    flake-parts.url = "github:hercules-ci/flake-parts";
    rust-overlay.url = "github:oxalica/rust-overlay";
    solana-toolchain.url = "github:hylo-so/solana-toolchain-nix";
  };

  outputs =
    inputs@{ self, nixpkgs, flake-parts, rust-overlay, solana-toolchain, }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems =
        [ "aarch64-darwin" "aarch64-linux" "x86_64-darwin" "x86_64-linux" ];

      perSystem = { pkgs, system, ... }:
        with import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        let
          sharedBuildInputs = [ libiconv pkg-config gcc openssl ];

          rust = rust-bin.stable."1.88.0".default.override {
            extensions = [ "rust-analyzer" "rust-src" ];
          };

          anchor = solana-toolchain.packages.${system}.anchor;

          cargoWrapper = import "${solana-toolchain}/cargo-wrapper.nix" {
            inherit writeShellApplication;
            cargo = rust;
          };

          shellTools =
            import ./shell-tools.nix { inherit writeShellApplication; };

        in {
          devShells.default = mkShell {
            packages = [
              cargoWrapper
              rust
              solana-toolchain.packages.${system}.solana-cli
              anchor
              gnumake
              jq
            ] ++ builtins.attrValues shellTools;

            buildInputs = sharedBuildInputs;

            CARGO_FEATURE_NO_NEON = "true";
          };

          packages = shellTools;

          devShells.sbf = mkShell {
            inputsFrom = [ solana-toolchain.devShells.${system}.default ];
            packages = [ cargoWrapper rust anchor gnumake ];

            buildInputs = sharedBuildInputs;

            CARGO_FEATURE_NO_NEON = "true";
          };

          devShells.nightly = mkShell {
            packages = [ rust-bin.nightly."2026-02-02".default ];
            buildInputs = sharedBuildInputs;
          };
        };
    };
}
