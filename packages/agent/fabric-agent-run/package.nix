{
  id = "fabric-agent-run";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = {
    prefix = "fabric-agent-run";
  };
}
