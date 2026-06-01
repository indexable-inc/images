{
  id = "launchk";
  # launchk observes launchd jobs over XPC, so it only builds on macOS (see
  # meta.platforms in default.nix). Advertising the flake output or a non-darwin
  # package-set attr makes `nix flake check` force a package nixpkgs refuses to
  # evaluate off-platform, so gate both to Darwin.
  packageSet.systems = [
    "aarch64-darwin"
    "x86_64-darwin"
  ];
  flake.systems = [
    "aarch64-darwin"
    "x86_64-darwin"
  ];
}
