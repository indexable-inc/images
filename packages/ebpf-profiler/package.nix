{
  id = "ebpf-profiler";
  # The profiler is a kernel-facing eBPF agent: it loads BPF programs, reads
  # /proc and unwinds native stacks, so it only builds on Linux (see
  # meta.platforms in default.nix). Advertising the flake output or the darwin
  # package-set attr makes `nix flake check` force a package nixpkgs refuses to
  # evaluate off-platform, so gate both to Linux.
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
