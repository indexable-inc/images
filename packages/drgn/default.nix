{
  lib,
  ix,
  python3,
  pkg-config,
  elfutils,
  autoconf,
  automake,
  libtool,
  gnumake,
  gcc,
}:
python3.pkgs.buildPythonApplication (finalAttrs: {
  pname = "drgn";
  version = "0.2.0";
  pyproject = true;

  src = ix.drgnSrc;

  build-system = [python3.pkgs.setuptools];

  # setup.py shells out to autotools (`autoreconf -i`, `./configure`, `make`)
  # via build_ext, so the autoconf/automake/libtool/pkg-config quartet plus
  # gcc and gnumake all have to be in scope. libdrgn's optional features
  # (libkdumpfile core dumps, libdebuginfod, lzma, pcre2, json-c) auto-detect
  # and stay disabled when their libs are absent. Live struct-graph traversal
  # over /proc/kcore (the workload this image cares about) only needs libelf
  # and libdw, both shipped by elfutils.
  nativeBuildInputs = [
    autoconf
    automake
    gcc
    gnumake
    libtool
    pkg-config
  ];

  buildInputs = [elfutils];

  meta = {
    description = "Programmable debugger for live processes and kernels";
    longDescription = ''
      drgn is a programmable debugger from Meta. It complements pahole's
      type-layout queries with live struct-graph traversal: dereferencing
      pointers, walking intrusive lists, and dumping fields off typed roots
      in a running process or kernel over /proc/kcore.
    '';
    homepage = "https://github.com/osandov/drgn";
    changelog = "https://github.com/osandov/drgn/releases/tag/v${finalAttrs.version}";
    license = lib.licenses.lgpl21Plus;
    mainProgram = "drgn";
    platforms = [
      "x86_64-linux"
      "aarch64-linux"
    ];
  };
})
