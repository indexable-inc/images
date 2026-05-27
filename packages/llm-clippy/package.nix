{
  id = "llm-clippy";
  packageSet = true;
  flake = true;
  callPackageArgs =
    {
      pkgs,
      rustNightlyClippyToolchainFor,
      clippy-fork,
      ...
    }:
    {
      rustToolchain = rustNightlyClippyToolchainFor pkgs;
      src = clippy-fork;
    };
}
