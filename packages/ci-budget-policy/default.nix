{
  ix,
  pkgs ? ix.pkgs,
}: let
  policyRoot = ix.paths.root + "/.github/actions/ci-budget";
  policy = policyRoot + "/catalog/policy.json";
  application = ix.writePythonApplication pkgs {
    name = "ci-budget-policy";
    src = policyRoot + "/ci_policy.py";
    args = [
      "--policy"
      policy
    ];
    pyChecker = "zuban";
    meta.description = "Classify one GitHub Actions run with the shared CI budget policy";
  };
in
  application.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit policy;
      };
  })
