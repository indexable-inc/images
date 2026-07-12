{
  ix,
  lib,
  pkgs,
  ...
}: let
  meta = {
    description = "View the repo's vendored-dependency patches and their upstream PR status";
    license = lib.licenses.mit;
    mainProgram = "prs";
  };

  unwrapped = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "prs";
    inherit meta;
  };

  # The same rendered registry upstream-sync bakes in: the binary reads the
  # fork mapping from this JSON via PRS_FORK_MAPPING (a checkout is only
  # needed for the patch files themselves), so `nix run .#prs` works without
  # `nix` re-evaluating anything at runtime. `gh` is appended to PATH for the
  # `gh auth token` fallback when GITHUB_TOKEN/GH_TOKEN is unset.
  forkData = (pkgs.formats.json {}).generate "fork-packages.json" ix.forkPackages;

  wrapped =
    pkgs.runCommand "prs"
    {
      nativeBuildInputs = [pkgs.makeWrapper];
      strictDeps = true;
      inherit meta;
    }
    ''
      mkdir -p $out/bin
      makeWrapper ${lib.getExe unwrapped} $out/bin/prs \
        --set-default PRS_FORK_MAPPING ${forkData} \
        --suffix PATH : ${lib.makeBinPath [pkgs.gh]}
    '';

  printsHelp =
    pkgs.runCommand "prs-prints-help"
    {
      nativeBuildInputs = [wrapped];
      strictDeps = true;
    }
    ''
      # No terminal, no token: --help must exit 0 and print usage.
      help=$(prs --help)
      case "$help" in
        *"Usage: prs"*) ;;
        *)
          echo "prs --help did not print usage" >&2
          printf '%s\n' "$help" >&2
          exit 1
          ;;
      esac
      mkdir -p "$out"
    '';
in
  wrapped.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (unwrapped.passthru.tests or {})
          // {
            inherit printsHelp;
          };
      };
  })
