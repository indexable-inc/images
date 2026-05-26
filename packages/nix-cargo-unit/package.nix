{
  id = "nix-cargo-unit";
  packageSet = true;
  flake = true;
  inRustWorkspace = true;
  passthruTests = true;
  callPackageArgs =
    { ix, pkgs, ... }:
    {
      inherit ix pkgs;
    };
}
