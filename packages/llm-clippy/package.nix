{
  id = "llm-clippy";
  packageSet = true;
  flake = true;
  callPackageArgs =
    {
      ixForPackages,
      clippy-fork,
      ...
    }:
    {
      ix = ixForPackages;
      src = clippy-fork;
    };
}
