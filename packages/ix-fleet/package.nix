{
  id = "ix-fleet";
  packageSet = true;
  flake = true;
  callPackageArgs =
    { ix, ... }:
    {
      inherit ix;
    };
}
