{
  id = "ix";
  packageSet = true;
  flake = true;
  callPackageArgs =
    { cliArtifacts, packageSystem, ... }:
    if builtins.hasAttr packageSystem cliArtifacts then
      {
        binarySrc = cliArtifacts.${packageSystem};
      }
    else
      { };
}
