{
  id = "nix-eval-jobs";
  # Surfaced as `pkgs.nix-eval-jobs` (overlay) and the `nix-eval-jobs` flake
  # output so the `check` app can hand its store path to nix-fast-build via
  # --nix-eval-jobs. x86_64-linux only: it is the CI build/eval system, and the
  # override is a heavy nix-against-libstore C++ rebuild not worth doing on
  # systems that never run the gate.
  packageSet = {
    systems = [ "x86_64-linux" ];
  };
  flake = {
    systems = [ "x86_64-linux" ];
  };
  overlay = {
    systems = [ "x86_64-linux" ];
  };
  passthruTests = true;
}
