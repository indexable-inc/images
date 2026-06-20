{
  ix,
  lib,
  pkgs,
}:

let
  # The house `claude` (wrapped) for the rollouts, plus git for the fake-clone
  # shim and coreutils for the shim's cp/basename. The eval passes an explicit
  # --system-prompt-file, which wins over the wrapper's baked prompt.
  claudeBin = pkgs.claude-code;

  # Datasets and fixtures ship as plain files: the first-principles eval copies a
  # fixture repo at runtime, and the behaviors eval reads JSONL task sets.
  data = pkgs.runCommand "system-prompt-eval-data" { strictDeps = true; } ''
    mkdir -p "$out/datasets" "$out/fixtures"
    cp -r ${./datasets}/. "$out/datasets/"
    cp -r ${./fixtures}/. "$out/fixtures/"
  '';

  # The exact committed house prompt, rendered to text at build time so
  # `nix run .#system-prompt-eval` tests precisely what ships. A candidate edit
  # is tested with --system-prompt-file / --system-prompt-nix instead.
  promptFile = pkgs.writeText "house-system-prompt.txt" (
    import ../system-prompt.nix { inherit lib; }
  );

  unwrapped = ix.buildUvApplication pkgs {
    pname = "system-prompt-eval";
    version = "0.1.0";
    srcRoot = ./.;
    mainProgram = "system-prompt-eval";
    pyChecker = "zuban";
    meta = {
      description = "Behavioral eval registry for the house system prompt";
      license = lib.licenses.mit;
      mainProgram = "system-prompt-eval";
    };
  };

  package =
    pkgs.runCommand "system-prompt-eval"
      {
        nativeBuildInputs = [ pkgs.makeWrapper ];
        strictDeps = true;
        meta = {
          description = "Behavioral eval registry for the house system prompt";
          license = lib.licenses.mit;
          mainProgram = "system-prompt-eval";
        };
      }
      ''
        mkdir -p $out/bin
        makeWrapper ${lib.getExe unwrapped} $out/bin/system-prompt-eval \
          --prefix PATH : ${
            lib.makeBinPath [
              claudeBin
              pkgs.git
              pkgs.coreutils
            ]
          } \
          --set SYSTEM_PROMPT_EVAL_DATA_DIR ${data} \
          --set SYSTEM_PROMPT_EVAL_PROMPT_FILE ${promptFile}
      '';

  # Offline, deterministic: the scoring math is the CI-gating signal, unit-tested
  # against the installed package with no network or key.
  scoring =
    pkgs.runCommand "system-prompt-eval-scoring" { strictDeps = true; } ''
      ${unwrapped}/venv/bin/python ${./tests/test_scoring.py}
      mkdir -p "$out"
    '';

  # No network and no API key: `list` and `--help` must work and name the program.
  printsHelp =
    pkgs.runCommand "system-prompt-eval-prints-help"
      {
        nativeBuildInputs = [ package ];
        strictDeps = true;
      }
      ''
        help=$(system-prompt-eval --help)
        case "$help" in
          *"system-prompt-eval"*) ;;
          *)
            echo "system-prompt-eval --help did not print usage" >&2
            printf '%s\n' "$help" >&2
            exit 1
            ;;
        esac
        system-prompt-eval list | grep -q behaviors
        system-prompt-eval list | grep -q first-principles
        mkdir -p "$out"
      '';
in
package.overrideAttrs (old: {
  passthru = (old.passthru or { }) // {
    tests = {
      inherit scoring printsHelp;
    };
  };
})
