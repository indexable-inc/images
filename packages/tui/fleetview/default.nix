{
  ix,
  lib,
  pkgs,
  ...
}: let
  meta = {
    description = "Terminal fleet view for Claude Code sessions: dispatch a task, watch every agent's PTY for activity, attach to one full-screen";
    license = lib.licenses.mit;
    mainProgram = "fleetview";
  };

  # Deliberately unwrapped: fleetview dispatches whatever `claude` (or
  # `--command`) resolves to on the user's PATH, which is the point -- pinning
  # an agent build into the wrapper would drive a different Claude than the one
  # the user is logged into.
  fleetview = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "fleetview";
    inherit meta;
  };

  printsHelp =
    pkgs.runCommand "fleetview-prints-help"
    {
      nativeBuildInputs = [fleetview];
      strictDeps = true;
    }
    ''
      # There is no TTY in the sandbox, so --help is the one path that must
      # still work: it parses argv and exits before taking over the terminal.
      help=$(fleetview --help)
      case "$help" in
        *"Usage: fleetview"*) ;;
        *)
          echo "fleetview --help did not print usage" >&2
          printf '%s\n' "$help" >&2
          exit 1
          ;;
      esac
      mkdir -p "$out"
    '';
in
  fleetview.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit printsHelp;
          };
      };
  })
