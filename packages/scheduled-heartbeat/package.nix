{
  id = "scheduled-heartbeat";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = {
    prefix = "scheduled-heartbeat";
  };
}
