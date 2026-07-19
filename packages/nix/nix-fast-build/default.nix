{
  ix,
  lib,
  pkgs,
}: let
  # Upstream Mic92/nix-fast-build (nix-fast-build-src, pinned to the tag
  # nixpkgs packages) with the in-repo patch series (./patches) applied:
  # --skip-cached skipping locally-realized outputs, and the typed
  # per-derivation no-progress deadline. The series, its upstream intent, and
  # the seconds-fast `patched-src-nix-fast-build` apply gate are driven by
  # lib/fork-packages.nix; each patch's commit message carries its full WHY.
  patchedSrc = ix.patchedSrc {
    name = "nix-fast-build";
    src = ix.nix-fast-buildSrc;
    patchDir = ./patches;
  };

  # The nixpkgs recipe expects nix-fast-build's version to match the source
  # it wraps; a nixpkgs bump with a stale nix-fast-build-src pin would
  # silently build the old tree under the new label, so fail eval until the
  # pin is advanced.
  package = assert lib.assertMsg (pkgs.nix-fast-build.version == "1.6.0") ''
    packages/nix/nix-fast-build: nixpkgs nix-fast-build is
    ${pkgs.nix-fast-build.version} but nix-fast-build-src pins tag 1.6.0.
    Repin the nix-fast-build-src input to the matching upstream tag and run
    `nix run .#rebase-patches -- nix-fast-build`.'';
    pkgs.nix-fast-build.overrideAttrs (old: {
      src = patchedSrc;
      doCheck = true;
      nativeCheckInputs =
        (old.nativeCheckInputs or [])
        ++ [
          pkgs.mypy
          pkgs.python3Packages.pytest
          pkgs.ruff
        ];
      checkPhase = ''
        # shell
        runHook preCheck
        pytest -q tests/test_liveness.py
        ruff check nix_fast_build
        mypy nix_fast_build
        runHook postCheck
      '';
    });

  # The patches only touch Python control flow, so the real risk is that the
  # surrounding source drifted out from under the series on a base bump --
  # `checks.<system>.patched-src-nix-fast-build` (and a build of `package`)
  # already catches that. The smoke test additionally runs the binary so an
  # import-time break surfaces here rather than mid-CI-run; `--help` exits 0
  # without touching a store or daemon (absent in the sandbox).
  smoke =
    pkgs.runCommand "nix-fast-build-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      help=$(nix-fast-build --help 2>&1) || true
      # --fail-fast is what the check gate passes (lib/per-system.nix); its
      # absence from usage is exactly the failure mode that broke CI when the
      # flag was assumed present on 1.5.0 (#2128), so assert both flags.
      case "$help" in
        *"--skip-cached"*) ;;
        *)
          echo "nix-fast-build --help did not print usage" >&2
          printf '%s\n' "$help" >&2
          exit 1
          ;;
      esac
      case "$help" in
        *"--fail-fast"*) ;;
        *)
          echo "nix-fast-build --help lacks --fail-fast (version < 1.6.0?)" >&2
          printf '%s\n' "$help" >&2
          exit 1
          ;;
      esac
      case "$help" in
        *"--max-no-progress-seconds"*) ;;
        *)
          echo "nix-fast-build --help lacks the per-derivation liveness policy" >&2
          printf '%s\n' "$help" >&2
          exit 1
          ;;
      esac
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit smoke;
          };
      };
    meta =
      (old.meta or {})
      // {
        description = "nix-fast-build with local-cache skipping and typed per-derivation liveness deadlines";
        mainProgram = "nix-fast-build";
      };
  })
