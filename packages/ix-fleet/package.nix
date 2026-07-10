{
  id = "ix-fleet";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = {
    prefix = "ix-fleet";
  };
}
