{
  ix,
  pkgs ? ix.pkgs,
}: let
  policyRoot = ix.paths.root + "/.github/actions/ci-budget";
  application = ix.writePythonApplication pkgs {
    name = "ci-budget-policy";
    src = policyRoot + "/resolver.py";
    runtimeImportRoots = [policyRoot];
    pyChecker = "zuban";
    meta.description = "Resolve one GitHub Actions run with the shared CI budget policy";
  };
in
  application.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        policy = policyRoot + "/policy.json";
        tests.runtime-imports = pkgs.runCommand "ci-budget-policy-runtime-imports" {
          nativeBuildInputs = [application pkgs.jq];
        } ''
          cd "$TMPDIR"
          printf '%s\n' '{"changed_paths":[],"force_big_change":false,"labels":[],"repository":"indexable-inc/ix","workflow_path":".github/workflows/ci.yml"}' \
            | ci-budget-policy > decision.json
          jq --exit-status \
            '.managed_workflow == true and .budget_seconds == 300' \
            decision.json
          touch "$out"
        '';
      };
  })
