{
  ix,
  lib,
  pkgs,
  ...
}: let
  workflowBase =
    pkgs.runCommand "clone-workflow-base-test" {
      nativeBuildInputs = [
        pkgs.bash
        pkgs.coreutils
        pkgs.git
        pkgs.yq-go
      ];
      strictDeps = true;
    } ''
      export HOME="$TMPDIR/home"
      mkdir -p "$HOME"
      bash ${./tests/workflow-base.sh} ${ix.paths.root + "/.github/workflows/check.yml"}
      mkdir -p "$out"
    '';
in
  ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "clone";
    packageName = "clone-cli";
    passthru.tests.workflow-base = workflowBase;
    meta = {
      description = "Code clone and duplication detector (Type-1/2/3) over tree-sitter ASTs";
      license = lib.licenses.mit;
      mainProgram = "clone";
    };
  }
