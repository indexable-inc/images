{
  id = "ci-budget-policy";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = {
    prefix = "ci-budget-policy";
  };
}
