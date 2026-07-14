# The repo lint gate. Each stage is one subcommand on the single `lint-stage`
# binary (crate `packages/lint`, src/stage.rs) so the dag spec keys off one
# executable without registering ten sibling packages; `lint` runs the spec
# via dag-runner (default) or collects the same nodes as one JSON document
# (`--json`, #1683).
{
  ix,
  pkgs,
  repoPackages,
  ...
}: let
  inherit (pkgs) lib;

  lintStageUnit = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "lint-stage";
    packageName = "lint";
    # The crate's #[test] checks ride the `lint` selection below; duplicating
    # them here would register every case twice.
    includeTestCases = false;
    meta.mainProgram = "lint-stage";
  };

  # The stage tools ride the wrapper PATH (not ambient) so `nix run .#lint`
  # pins the exact linter versions CI runs. File discovery (`fd` before the
  # Rust port) is the crate's `ignore`-based walker, so no fd here. The ruff
  # selector argv is handed over as JSON (ix.ruffAnnArgv), not a shell
  # fragment: the stage execs ruff directly.
  lintStage = ix.wrapPackage pkgs {
    package = lintStageUnit;
    pathSuffix = [
      pkgs.alejandra
      pkgs.deadnix
      pkgs.ruff
      pkgs.statix
      repoPackages.astlog
      repoPackages.clone
    ];
    env.IX_RUFF_ARGV = builtins.toJSON ix.ruffAnnArgv;
    meta = {
      description = "One lint stage (alejandra | statix | deadnix | astlog | astlog-rust | astlog-elixir | filenames | dirnames | ruff | clone); driven by `lint`";
      mainProgram = "lint-stage";
    };
  };

  # One stage list drives both the dag spec (default human path) and the
  # `--json` runner inside `lint` (it reads this same generated spec), so
  # adding a stage cannot update one path and silently miss the other. The
  # `stage-list` passthru test below pins this list to the binary's own
  # stage enum.
  stages = [
    "alejandra"
    "statix"
    "deadnix"
    "astlog"
    "astlog-rust"
    "astlog-elixir"
    "filenames"
    "dirnames"
    "ruff"
    "clone"
  ];

  spec = (pkgs.formats.json {}).generate "lint-dag.json" {
    nodes = lib.genAttrs stages (stage: {
      command = [
        (lib.getExe lintStage)
        stage
      ];
    });
  };

  lintUnit = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "lint";
    meta.mainProgram = "lint";
  };
in
  ix.wrapPackage pkgs {
    package = lintUnit;
    env = {
      IX_LINT_SPEC = "${spec}";
      IX_DAG_RUNNER = lib.getExe repoPackages.dag-runner;
    };
    passthru = {
      # The per-system filename-policy / dirname-policy checks exercise single
      # stages in synthetic trees; export the wrapped stage binary for them.
      inherit lintStage;
      tests =
        lintUnit.passthru.tests
        // {
          # The nix-side `stages` list above and the crate's Stage enum are two
          # statements of one fact; fail the build when they drift.
          stage-list = pkgs.runCommand "lint-stage-list-check" {} ''
            ${lib.getExe lintStage} --list | sort > got
            printf '%s\n' ${lib.escapeShellArgs stages} | sort > want
            diff -u want got
            mkdir -p "$out"
          '';
        };
    };
    meta = {
      description = "Run all Nix formatting and lint checks in parallel via dag-runner";
      mainProgram = "lint";
    };
  }
