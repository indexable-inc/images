{
  id = "llm-clippy";
  packageSet = true;
  flake = true;
  callPackageArgs =
    {
      pkgs,
      ixForPackages,
      rustNightlyClippyToolchainFor,
      clippy-fork,
      ...
    }:
    {
      ix = ixForPackages;
      rustToolchain = rustNightlyClippyToolchainFor pkgs;
      src = clippy-fork;
    };
}
