{
  id = "spark-gluten";
  # Upstream publishes only a linux_amd64 native build of the Velox/Arrow
  # libraries, and the package autopatchelfs ELF objects, so it builds only on
  # x86_64-linux (see meta.platforms in default.nix). Advertising the flake
  # output or an off-platform package-set attr makes `nix flake check` force a
  # build nixpkgs refuses to evaluate, so gate both to x86_64-linux. The overlay
  # stays unconditional and lazy: `pkgs.spark-gluten` is only forced inside an
  # x86_64-linux closure (the spark service module), mirroring how `drgn` does
  # the same for its Linux-only binary.
  packageSet.systems = [ "x86_64-linux" ];
  flake.systems = [ "x86_64-linux" ];
  overlay = true;
}
