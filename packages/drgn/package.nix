{
  id = "drgn";
  # drgn debugs live processes and the kernel over /proc/kcore, so it only
  # builds on Linux (see meta.platforms in default.nix). Advertising the flake
  # output or the darwin package-set attr makes `nix flake check` force a
  # package nixpkgs refuses to evaluate off-platform, so gate both to Linux.
  # The overlay stays unconditional: an `overlay.systems` filter would force
  # `final.stdenv.hostPlatform.system` while building the overlay's own
  # attrset spine and infinite-loops. `pkgs.drgn` on darwin is lazy and only
  # the Linux base profile (an x86_64-linux closure) ever forces it.
  packageSet.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  flake.systems = [
    "x86_64-linux"
    "aarch64-linux"
  ];
  overlay = true;
}
