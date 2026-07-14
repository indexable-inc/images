{
  id = "rebase-patches";
  packageSet = true;
  flake = true;
  overlay = false;
  inRustWorkspace = true;
  passthruTests = {prefix = "rebase-patches";};
}
