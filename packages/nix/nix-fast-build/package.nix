{
  id = "nix-fast-build";
  # Surfaced in the repo package set (`repoPackages.nix-fast-build`, which the
  # `check` app invokes directly) and as the `nix-fast-build` flake output.
  # Deliberately NOT in the nixpkgs overlay: the override reads
  # `pkgs.nix-fast-build` as its base, so injecting this package under the same
  # name would make it its own base (infinite recursion) -- same reasoning as
  # packages/nix/nix-eval-jobs. x86_64-linux only: it is the CI build system and
  # the only consumer is the x86_64-linux-only `check` app.
  packageSet = {
    systems = ["x86_64-linux"];
  };
  flake = {
    systems = ["x86_64-linux"];
  };
  passthruTests = true;
}
