{
  id = "mcp";
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
