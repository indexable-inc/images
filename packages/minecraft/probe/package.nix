{
  id = "mc-probe";
  packageSet = true;
  flake = true;
  callPackageArgs =
    { ix, ... }:
    {
      inherit ix;
    };
}
