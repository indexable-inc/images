{
  id = "nix-output-monitor";
  packageSet = true;
  flake = true;
  overlay = {
    build = {
      ix,
      lib,
      path,
      prev,
      ...
    }:
      lib.callPackageWith prev path {
        pkgs = prev;
        # `prev` has no `ix` attr, so thread the overlay context's handle
        # (which carries the nix-derivation-src input and patchedSrc).
        inherit ix;
      };
  };
  passthruTests = true;
}
