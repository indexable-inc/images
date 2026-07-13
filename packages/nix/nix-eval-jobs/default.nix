{
  pkgs,
  repoPackages,
}: let
  # CA realisations are an unstable protocol. Build the evaluator from the
  # same pinned 2.34 component set as the fleet daemon. A separate fetch here
  # previously drifted to Nix master while the daemon stayed on 2.34.7,
  # breaking CA realisation negotiation.
  #
  # Specifically the PATCHED component set (nix-ix, packages/nix/nix), not
  # stock `nixComponents_2_34`: this is the evaluator that parses every repo
  # .nix file in CI (nix-fast-build drives it over ciChecks and the package
  # eval gate), so it must speak the same dialect the patched client and
  # daemon family do -- underscore digit separators included.
  package = pkgs.nix-eval-jobs.override {
    nixComponents = repoPackages.nix-ix.passthru.components;
  };

  # The override's real risk is silently relinking against nixpkgs' default
  # Nix family after an update, so the smoke test checks both the executable
  # and its propagated Nix component version -- including the `+ix` build-metadata marker,
  # so a silent fallback to the stock 2.34 components fails here.
  smoke =
    pkgs.runCommand "nix-eval-jobs-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      case ${package.nixComponents.nix-cli.version} in
        2.34.*+ix*) ;;
        *)
          echo "nix-eval-jobs is not linked to the patched 2.34 (+ix) component family" >&2
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
        description = "nix-eval-jobs built against the patched nix-ix 2.34 components (underscore digit separators)";
        mainProgram = "nix-eval-jobs";
      };
  })
