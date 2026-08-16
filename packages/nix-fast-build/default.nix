{
  ix,
  lib,
  pkgs,
}: let
  # The nix-fast-build view carries --skip-cached handling for locally realized
  # outputs and the typed per-derivation no-progress deadline.
  patchedSrc = ix.nix-fast-buildSrc;

  # The nixpkgs recipe expects nix-fast-build's version to match the source
  # it wraps; a nixpkgs bump with a stale view would build the old tree under
  # the new label, so fail eval until the view advances.
  package = assert lib.assertMsg (pkgs.nix-fast-build.version == "1.6.0") ''
    packages/nix-fast-build: nixpkgs nix-fast-build is
    ${pkgs.nix-fast-build.version} but the view is tag 1.6.0.
    Update the nix-fast-build view to the matching upstream tag.'';
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

  # The fork commits only touch Python control flow. Building `package` catches
  # source drift. The smoke test additionally runs the binary so an
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
