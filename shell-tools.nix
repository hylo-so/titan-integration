{ writeShellApplication }: {
  lint = writeShellApplication {
    name = "lint";
    text = ''
      nix develop .#nightly --command bash -c "
        set -euo pipefail
        cargo-fmt --check
        cargo-clippy --check
        cargo-fmt --check --manifest-path program-template/Cargo.toml
        cargo-clippy --check --manifest-path program-template/Cargo.toml --tests
      "
    '';
  };

  polish = writeShellApplication {
    name = "polish";
    text = ''
      nix develop .#nightly --command bash -c "
        set -euo pipefail
        cargo-clippy --fix
        cargo-fmt
        cargo-clippy --fix --manifest-path program-template/Cargo.toml --tests
        cargo-fmt --manifest-path program-template/Cargo.toml
      "
    '';
  };

  build = writeShellApplication {
    name = "build";
    text = "nix develop --command cargo build";
  };

  build-program = writeShellApplication {
    name = "build-program";
    text = ''
      nix develop .#sbf --command bash -c "
        set -euo pipefail
        cd program-template
        anchor build --no-idl -- --no-rustup-override --skip-tools-install
      "
    '';
  };

  test-structure = writeShellApplication {
    name = "test-structure";
    text = "nix develop --command make check-structure";
  };

  test-venue = writeShellApplication {
    name = "test-venue";
    text = ''
      nix develop --command bash -c "
        set -euo pipefail
        make dump-programs
        make test-venue
      "
    '';
  };
}
