{
  ix,
  lib,
  pkgs,
}: let
  fs = lib.fileset;
  src = fs.toSource {
    root = ./.;
    fileset = fs.unions [
      ./pyproject.toml
      ./src
      ./uv.lock
    ];
  };

  package = ix.buildUvApplication pkgs {
    pname = "fabric-agent-run";
    version = "0.1.0";
    inherit src;
    mainProgram = "fabric-agent-run";
    pyChecker = "zuban";
    meta = {
      description = "Call-owned Claude and Codex runs recorded in the Weave journal";
      license = lib.licenses.mit;
      mainProgram = "fabric-agent-run";
    };
  };

  unit =
    pkgs.runCommand "fabric-agent-run-unit"
    {
      strictDeps = true;
    }
    ''
      ${package}/venv/bin/python -m unittest discover -s ${./tests} -v
      mkdir -p "$out"
    '';

  printsHelp =
    pkgs.runCommand "fabric-agent-run-prints-help"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      help=$(fabric-agent-run --help)
      case "$help" in
        *"Run one Claude or Codex call"*) ;;
        *)
          echo "fabric-agent-run --help did not describe the command" >&2
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
        tests = {
          inherit printsHelp unit;
        };
      };
  })
