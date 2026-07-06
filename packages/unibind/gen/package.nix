{
  id = "unibind-gen";
  packageSet = true;
  flake = true;
  overlay = false;
  inRustWorkspace = true;
  passthruTests = {
    prefix = "unibind-gen";
  };
}
