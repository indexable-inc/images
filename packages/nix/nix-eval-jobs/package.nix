{
  id = "nix-eval-jobs";
  # Surfaced in the repo package set (`repoPackages.nix-eval-jobs`, which the
  # `check` app hands to nix-fast-build via --nix-eval-jobs) and as the
  # `nix-eval-jobs` flake output. Deliberately NOT in the nixpkgs overlay: the
  # override reads `pkgs.nix-eval-jobs` as its base, so injecting this package
  # under the same name would make it its own base (infinite recursion).
  # x86_64-linux only: it is the CI build/eval system, and the override is a
  # heavy nix-against-libstore C++ rebuild not worth doing elsewhere.
  packageSet = {
    systems = [ "x86_64-linux" ];
  };
  flake = {
    systems = [ "x86_64-linux" ];
  };
  passthruTests = true;
}
