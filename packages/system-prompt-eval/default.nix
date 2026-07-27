{
  ix,
  lib,
  pkgs,
}: let
  # `claude` is resolved from the runtime PATH (not baked): baking pkgs.claude-code
  # would pull the x86_64-linux rust workspace and break the build on darwin, and
  # the eval should anyway exercise whatever `claude` the runner actually has. The
  # eval passes an explicit --system-prompt-file, which wins over claude's baked
  # prompt. git (for the fake-clone shim) and coreutils ARE baked.
  # Datasets and fixtures ship as plain files: the first-principles eval copies a
  # fixture repo at runtime, and the behaviors eval reads JSONL task sets.
  data = pkgs.runCommand "system-prompt-eval-data" {strictDeps = true;} ''
    mkdir -p "$out/datasets" "$out/fixtures"
    cp -r ${./datasets}/. "$out/datasets/"
    cp -r ${./fixtures}/. "$out/fixtures/"
  '';

  # The exact committed house prompt, rendered to text at build time so
  # `nix run .#system-prompt-eval` tests precisely what ships. A candidate edit
  # is tested with --system-prompt-file / --system-prompt-nix instead.
  promptFile =
    pkgs.writeText "house-system-prompt.txt"
    (import (ix.paths.packagesRoot + "/agent-prompt") {inherit lib;}).systemPrompt;

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
      nativeBuildInputs = [pkgs.makeWrapper];
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
          pkgs.git
          pkgs.coreutils
        ]
      } \
        --set SYSTEM_PROMPT_EVAL_DATA_DIR ${data} \
        --set SYSTEM_PROMPT_EVAL_PROMPT_FILE ${promptFile}
    '';

  # Offline, deterministic: the scoring math is the CI-gating signal, unit-tested
  # against the installed package with no network or key. SYSTEM_PROMPT_EVAL_DATA_DIR
  # points the dataset-integrity check (validate_expects over the committed JSONL)
  # at the real datasets, the same way the wrapped `package` does at runtime.
  scoring = pkgs.runCommand "system-prompt-eval-scoring" {strictDeps = true;} ''
    SYSTEM_PROMPT_EVAL_DATA_DIR=${data} ${unwrapped}/venv/bin/python ${./tests/test_scoring.py}
    mkdir -p "$out"
  '';

  # No network and no API key: `list` and `--help` must work and name the program.
  printsHelp =
    pkgs.runCommand "system-prompt-eval-prints-help"
    {
      nativeBuildInputs = [package];
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
  # The workflow is this CLI's only caller that passes flags nothing else
  # exercises, so it drifted silently: `--html-out` outlived the flag it names
  # by the whole life of the viewer app, and every `/run-evals` since exited 2
  # on "unrecognized arguments" with no one watching. Assert every long flag the
  # workflow's eval step passes is one `run --help` still lists.
  workflowSource = lib.fileset.toSource {
    inherit (ix.paths) root;
    fileset = ix.paths.root + "/.github/workflows/system-prompt-eval.yml";
  };

  workflowFlags =
    pkgs.runCommand "system-prompt-eval-workflow-flags"
    {
      nativeBuildInputs = [
        package
        pkgs.yq-go
      ];
      strictDeps = true;
    }
    ''
      step=$(yq '.jobs.run-evals.steps[] | select(.name == "Run the eval").run' \
        ${workflowSource}/.github/workflows/system-prompt-eval.yml)
      flags=$(printf '%s' "$step" | grep -o -- '--[a-z][a-z-]*' | sort -u)
      if [ -z "$flags" ]; then
        echo "no flags found in the workflow's eval step; this check has drifted" >&2
        printf '%s\n' "$step" >&2
        exit 1
      fi
      help=$(system-prompt-eval run --help)
      for flag in $flags; do
        case "$help" in
          *"$flag"*) ;;
          *)
            echo "system-prompt-eval.yml passes $flag, which \`run\` does not accept" >&2
            printf '%s\n' "$help" >&2
            exit 1
            ;;
        esac
      done
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests = {
          inherit scoring printsHelp workflowFlags;
        };
      };
  })
