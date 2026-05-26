{
  id = "minecraft-rcon";
  overlay = {
    attrName = "minecraft-rcon";
    callPackageArgs =
      { writePythonApplication, ... }:
      {
        inherit writePythonApplication;
      };
  };
}
