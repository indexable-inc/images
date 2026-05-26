{
  id = "oci-image-builder";
  packageSet = true;
  flake = true;
  inRustWorkspace = true;
  passthruTests = true;
  callPackageArgs =
    { ix, ... }:
    {
      inherit ix;
    };
  overlay = {
    attrName = "oci-image-builder";
    build =
      {
        buildIxRustTool,
        final,
        path,
        ...
      }:
      buildIxRustTool final path;
  };
}
