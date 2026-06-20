{
  lib,
  ix,
  stdenv,
  autoreconfHook,
  pkg-config,
  rdma-core,
  pciutils,
}:

stdenv.mkDerivation {
  pname = "perftest";
  version = "26.04.17";

  src = ix.perftestSrc;

  # perftest ships autogen.sh (an `autoreconf -i` wrapper) rather than a
  # committed ./configure, so autoreconfHook regenerates the build system.
  nativeBuildInputs = [
    autoreconfHook
    pkg-config
  ];

  # libibverbs + librdmacm (+ libibumad) come from rdma-core; libpci from
  # pciutils feeds the optional PCI-relative-path reporting. CUDA/ROCm GPUDirect
  # paths auto-detect and stay off without their toolkits, which is what we want
  # for a plain RoCE latency/bandwidth benchmark.
  buildInputs = [
    rdma-core
    pciutils
  ];

  meta = {
    description = "RDMA (RoCE/InfiniBand) verbs and connection-manager latency/bandwidth benchmarks";
    longDescription = ''
      linux-rdma perftest: ib_send_bw / ib_write_bw / ib_read_bw and the matching
      *_lat tools, plus raw_ethernet_*. Used to measure RoCEv2 one-way latency and
      multi-QP aggregate bandwidth across the fleet vRack, e.g. a single process
      driving N queue pairs over a LACP bond: `ib_send_bw -R -q 8 -D 10 <peer>`.
      The suite installs many binaries under bin/; mainProgram points at
      ib_send_bw so `nix run .#perftest -- ...` runs the multi-QP bandwidth tool,
      while the rest are reachable via `nix shell .#perftest`.
    '';
    homepage = "https://github.com/linux-rdma/perftest";
    # Dual-licensed GPLv2-or-BSD-2 (see COPYING); record both.
    license = [
      lib.licenses.gpl2Only
      lib.licenses.bsd2
    ];
    mainProgram = "ib_send_bw";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
  };
}
