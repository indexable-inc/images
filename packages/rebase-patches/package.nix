{
  id = "rebase-patches";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = {prefix = "rebase-patches";};
}
