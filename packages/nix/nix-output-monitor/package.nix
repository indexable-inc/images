{
  id = "nix-output-monitor";
  packageSet = true;
  flake = true;
  overlay = {
    build = {
      lib,
      path,
      prev,
      ...
    }:
      lib.callPackageWith prev path {
        pkgs = prev;
      };
  };
  passthruTests = true;
}
