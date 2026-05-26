{
  id = "minestom-hello-server-jar";
  packageSet.attrPath = [
    "minestom"
    "helloServerJar"
  ];
  flake = true;
  callPackageArgs =
    { ix, ... }:
    {
      inherit ix;
    };
}
