{
  id = "minecraft-sync-managed";
  packageSet = true;
  flake = true;
  inRustWorkspace = true;
  passthruTests = true;
  callPackageArgs =
    { ix, ... }:
    {
      inherit ix;
    };
}
