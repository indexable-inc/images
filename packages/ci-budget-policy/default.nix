{
  ix,
  pkgs ? ix.pkgs,
}: let
  policyRoot = ix.paths.root + "/.github/actions/ci-budget";
  policy = policyRoot + "/catalog/policy.json";
  # The suites read sibling workflows through `parents[2]`, so they need the
  # whole .github tree, not just the action directory.
  githubRoot = ix.paths.root + "/.github";
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
  # Both suites existed and neither ran: no check referenced them, so
  # test_ci_policy.py had been failing on main against a workflow that moved to
  # a dispatcher claim, and nobody saw it. The catalog lint that keeps every
  # threshold's evidence beside it (ci_policy.check_notes) is only a guard if
  # something forces it, which is what these two derivations do.
  policyTests =
    pkgs.runCommandLocal "ci-budget-policy-tests" {
      nativeBuildInputs = [pkgs.python3];
    } ''
      cp -r ${githubRoot} github
      chmod -R u+w github
      cd github/actions/ci-budget
      python3 -m unittest test_ci_policy test_ci_budget
      touch "$out"
    '';
  workerTests =
    pkgs.runCommandLocal "ci-budget-worker-tests" {
      nativeBuildInputs = [pkgs.nodejs];
    } ''
      cp -r ${policyRoot} action
      chmod -R u+w action
      cd action/worker
      node --test
      touch "$out"
    '';
in
  application.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit policy;
        tests =
          (old.passthru.tests or {})
          // {
            policy = policyTests;
            worker = workerTests;
          };
      };
  })
