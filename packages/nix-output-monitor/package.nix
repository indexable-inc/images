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
  # Linux->Darwin cross lane (RFC 0009, #3606): CI cross-compiles nom via a
  # Linux-hosted cross GHC (ix.crossGhc) and aliases the Mach-O arm64 result
  # into `packages.aarch64-darwin.nix-output-monitor`, so Macs substitute it
  # from the cache instead of the native darwin lane building GHC + nom.
  cross = true;
  passthruTests = true;
}
