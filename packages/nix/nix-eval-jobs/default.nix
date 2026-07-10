{pkgs}: let
  # CA realisations are an unstable protocol. Build the evaluator against the
  # fleet daemon's Nix 2.34 protocol generation while the interactive client
  # remains independently selectable.
  package = pkgs.nix-eval-jobs.override {
    nixComponents = pkgs.nixVersions.nixComponents_2_34;
  };

  # The override's real risk is the C++ rebuild against nix's libstore linking,
  # so the smoke test runs the binary. `--help` exits 0 and prints usage without
  # contacting a store daemon, which is absent in the sandbox.
  smoke =
    pkgs.runCommand "nix-eval-jobs-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      help=$(nix-eval-jobs --help 2>&1) || true
      case "$help" in
        *"--check-cache-status"*) ;;
        *)
          echo "nix-eval-jobs --help did not print usage" >&2
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
        description = "nix-eval-jobs built against the fleet daemon's Nix protocol generation";
        mainProgram = "nix-eval-jobs";
      };
  })
