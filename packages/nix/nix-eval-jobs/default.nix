{pkgs}: let
  daemonNixSrc = pkgs.fetchFromGitHub {
    owner = "NixOS";
    repo = "nix";
    rev = "ac94798c753e48fd0b36128a029ed8aecebe9b56";
    hash = "sha256-lMvyBFy7jl8cnUI8efQuW8lxgIiwUhw6CHEmDpK0mfw=";
  };
  # CA realisations are an unstable protocol. Build the evaluator from the
  # exact Nix revision reported by the fleet daemon, while the interactive
  # client remains on the stable release.
  package =
    (pkgs.nix-eval-jobs.override {
      nixComponents = pkgs.nixVersions.nixComponents_git.overrideSource daemonNixSrc;
    }).overrideAttrs (old: {
      patches = (old.patches or []) ++ [./nix-master-api.patch];
    });

  # The override's real risk is the C++ rebuild against nix's libstore linking
  # and the new symbols (staticOutputHashes, getDefaultSubstituters,
  # Store::queryRealisation) resolving at all, so the smoke test runs the
  # binary. `--help` exits 0 and prints usage without touching a store or
  # daemon (absent in the sandbox).
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
