{
  id = "symphony-room-server";
  packageSet = true;
  flake = true;
  callPackageArgs =
    { pkgs, ... }:
    {
      inherit pkgs;
    };
}
