{
  id = "run";
  packageSet = true;
  flake = true;
  callPackageArgs =
    { ix, pkgs, ... }:
    {
      inherit ix pkgs;
    };
}
