{
  id = "perftest";
  # perftest builds the RDMA verbs/CM benchmarks (ib_send_bw, ib_write_lat, ...)
  # against libibverbs/librdmacm, so it is Linux-only. Gate the flake output and
  # the package-set attr to Linux for the same reason drgn does: advertising them
  # off-platform makes `nix flake check` force a build nixpkgs refuses to evaluate.
  # The overlay stays unconditional (lazy; only a Linux closure ever forces it).
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
