{
  lib,
  rustPlatform,
  runCommand,
  ix,
}:
# pdtpartners/nix-ninja: a ninja-compatible CLI that parses the build.ninja
# meson emits and creates one content-addressed derivation per compilation
# unit (Nix dynamic derivations). External-Rust-tool house style per
# skills/dependency-intake/SKILL.md; source pinned by rev as the
# `nix-ninja-src` flake input (pre-alpha upstream, deliberate bumps only).
#
# Two binaries ship from one build and must travel together: `nix-ninja`
# (the ninja replacement meson invokes via $NINJA) and `nix-ninja-task`
# (the in-sandbox builder every generated derivation execs, so it must
# resolve to a /nix/store path -- being in this output's bin/ satisfies
# that). Consumed by packages/nix-ninja-build, the incremental build
# lane for the patched nix fork (#3655).
let
  src = ix.nixNinjaSrc;
  nix-ninja = rustPlatform.buildRustPackage {
    pname = "nix-ninja";
    # No upstream release yet; nixpkgs unstable-version spelling of the pinned
    # rev's commit date.
    version = "0.1.0-unstable-2026-05-14";

    inherit src;

    cargoLock = {
      lockFile = src + "/Cargo.lock";
      # The lock carries three git dependencies (upstream's harmonia store
      # model plus the author's n2/igraph forks), so importCargoLock needs
      # their fixed-output hashes; refresh alongside a nix-ninja-src bump by
      # copying the corrected hashes from the fetchgit mismatch errors.
      outputHashes = {
        "harmonia-store-core-0.0.0-alpha.0" = "sha256-mLAOJjFm4AGp2GNE+rHBMsxlKOIaBobiK3wQW95PalE=";
        "include-graph-1.2.2" = "sha256-W5YuVwtdJnNbFY2PbcJ/pM0P6gL+jFZLjvxLnHuugvw=";
        "n2-0.1.0" = "sha256-TwpX/UdbQwfSmNlb6baGTKnCRW4FJPJ6FX9i4L2jYtU=";
      };
    };

    strictDeps = true;

    # Only the two binaries the lane needs; the workspace's other members
    # (deps-infer, nix-tool) ride along as path dependencies. Tests stay
    # workspace-wide: the only #[test]s are hermetic depfile/include parsers
    # in deps-infer.
    cargoBuildFlags = [
      "--package"
      "nix-ninja"
      "--package"
      "nix-ninja-task"
    ];

    passthru.tests = {
      # nix-ninja advertises itself to meson as ninja >= 1.8.2; the smoke test
      # pins that contract (meson refuses the $NINJA override below 1.8.2).
      smoke =
        runCommand "nix-ninja-smoke" {
          nativeBuildInputs = [nix-ninja];
          strictDeps = true;
        } ''
          version=$(nix-ninja --version)
          if [ "$version" != "1.8.2" ]; then
            echo "expected ninja-compat version 1.8.2, got: $version" >&2
            exit 1
          fi
          command -v nix-ninja-task >/dev/null
          mkdir -p "$out"
        '';
    };

    meta = {
      description = "Ninja-compatible incremental C/C++ builds via Nix dynamic derivations (one content-addressed derivation per compilation unit)";
      homepage = "https://github.com/pdtpartners/nix-ninja";
      license = lib.licenses.mit;
      mainProgram = "nix-ninja";
      # Upstream's support claim; see package.nix for the system gating.
      platforms = ["x86_64-linux"];
    };
  };
in
  nix-ninja
