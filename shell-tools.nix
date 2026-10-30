{ writeShellApplication, curl, git, jq, gnused }: {
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

  check-version = writeShellApplication {
    name = "check-version";
    runtimeInputs = [ git gnused ];
    text = ''
      manifest_version() {
        sed -n 's/^version = "\(.*\)"$/\1/p' | head -1
      }

      git fetch --quiet origin main
      current=$(manifest_version < Cargo.toml)
      base=$(git show origin/main:Cargo.toml | manifest_version)
      IFS=. read -r major minor patch <<< "$base"
      expected_patch="$major.$minor.$((patch + 1))"
      expected_minor="$major.$((minor + 1)).0"
      expected_major="$((major + 1)).0.0"

      case $current in
        "$expected_patch" | "$expected_minor" | "$expected_major")
          echo "Version $current is one bump above main ($base)." ;;
        *)
          echo "Version $current must be one patch, minor or major bump above main ($base):" >&2
          echo "  $expected_patch, $expected_minor or $expected_major" >&2
          exit 1 ;;
      esac
    '';
  };

  publish = writeShellApplication {
    name = "publish";
    runtimeInputs = [ curl jq gnused ];
    text = ''
      # Orders two MAJOR.MINOR.PATCH versions; prerelease and build
      # suffixes are dropped. Exit 0 when $1 > $2, 1 when equal, 2 when $1 < $2.
      semver_compare() {
        local IFS=.
        local -a left right
        read -r -a left <<< "''${1%%[-+]*}"
        read -r -a right <<< "''${2%%[-+]*}"
        for i in 0 1 2; do
          if ((10#''${left[i]:-0} > 10#''${right[i]:-0})); then return 0; fi
          if ((10#''${left[i]:-0} < 10#''${right[i]:-0})); then return 2; fi
        done
        return 1
      }

      crate=$(sed -n 's/^name = "\(.*\)"$/\1/p' Cargo.toml | head -1)
      local_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
      published=$(curl -s -A hylo-titan-publish \
        "https://crates.io/api/v1/crates/$crate" \
        | jq -r '.crate.max_version // "0.0.0"')

      set +e
      semver_compare "$local_version" "$published"
      order=$?
      set -e
      case $order in
        0) echo "$crate $local_version > $published on crates.io. Publishing." ;;
        1) echo "$crate $local_version already on crates.io. Nothing to publish."
           exit 0 ;;
        *) echo "$crate $local_version is older than $published on crates.io." >&2
           exit 1 ;;
      esac

      nix develop --command bash -c "
        set -euo pipefail
        cargo publish --package $crate
      "
    '';
  };
}
