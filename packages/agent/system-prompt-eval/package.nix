{
  id = "system-prompt-eval";
  packageSet = true;
  flake = true;
  # Without this the offline scoring/printsHelp tests in default.nix never reach
  # the required flake-check job: ciChecks only collects passthru.tests for
  # packages that declare passthruTests here. The non-rust prefix keeps the check
  # names as `system-prompt-eval-*` rather than the default `rust-*`.
  passthruTests = {
    prefix = "system-prompt-eval";
  };
}
