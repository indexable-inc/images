{
  id = "nix-ninja";
  # Upstream only supports x86_64-linux (its flake pins systems to that, and
  # nix-ninja-task fixes up build outputs with patchelf, an ELF-only tool), so
  # gate every surface to that system: advertising the package off-platform
  # would make `nix flake check` force a build upstream never claims to work.
  packageSet.systems = ["x86_64-linux"];
  flake.systems = ["x86_64-linux"];
  passthruTests = true;
}
