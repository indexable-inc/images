{
  id = "llm-clippy";
  packageSet = true;
  flake = true;
  callPackageArgs =
    { pkgs, rustNightlyClippyToolchainFor, ... }:
    {
      rustToolchain = rustNightlyClippyToolchainFor pkgs;
    };
}
