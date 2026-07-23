{
  id = "nix-ninja-build-nix";
  # The incremental (per-compilation-unit) build lane for the patched nix
  # fork. Gated to x86_64-linux because nix-ninja itself is (see
  # packages/nix-ninja/package.nix); the whole-package lanes on every other
  # system are untouched.
  packageSet.systems = ["x86_64-linux"];
  flake.systems = ["x86_64-linux"];
}
