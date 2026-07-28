{
  id = "dag-runner";
  packageSet = true;
  flake = true;
  overlay = false;
  inRustWorkspace = true;
  cross = true;
  passthruTests = true;
}
