{pkgs}: let
  # nix-eval-jobs uses libstore's worker protocol directly. Build it against
  # the same stable Nix family as the deployed fleet daemon, not nixpkgs'
  # moving default components: a newer client can require protocol features
  # the 2.34 daemon does not advertise and then fail floating-CA evaluation.
  package = pkgs.nix-eval-jobs.override {
    nixComponents = pkgs.nixVersions.nixComponents_2_34;
  };

  # The override's real risk is silently relinking against nixpkgs' default
  # Nix family after an update, so the smoke test checks both the executable
  # and its propagated Nix component version.
  smoke =
    pkgs.runCommand "nix-eval-jobs-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      case ${package.nixComponents.nix-cli.version} in
        2.34.*) ;;
        *)
          echo "nix-eval-jobs is not linked to the fleet daemon's Nix 2.34 family" >&2
          exit 1
          ;;
      esac
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
        description = "nix-eval-jobs built against the fleet daemon's stable Nix 2.34 protocol family";
        mainProgram = "nix-eval-jobs";
      };
  })
