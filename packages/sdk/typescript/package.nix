{
  id = "sdk-typescript";
  packageSet = true;
  flake = true;
  # Check-only derivation: the published artifact is the npm package, so the
  # nixpkgs overlay surface has nothing to offer here.
  overlay = false;
  passthruTests = {
    prefix = "sdk-typescript";
  };
}
