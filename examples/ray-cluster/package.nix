{
  ix,
  lib,
  pkgs ? ix.pkgs,
}:
let
  fs = lib.fileset;
  src = fs.toSource {
    root = ./.;
    fileset = fs.unions [
      ./pyproject.toml
      ./src
      ./uv.lock
    ];
  };
in
ix.buildUvApplication pkgs {
  pname = "ray-demo";
  version = "0.1.0";
  inherit src;
  # Ray's wheel bundles a compiled `_raylet.so` that the Python driver dlopens
  # at import; it needs a modern libstdc++ and zlib on the loader path. These
  # cover the driver (Ray client) process. The standalone `raylet`/`gcs_server`
  # daemons that `ray start` execs are FHS binaries served by the image's
  # `nix-ld`; the head/worker services point `NIX_LD*` at that bundle.
  runtimeLibraryInputs = [
    pkgs.stdenv.cc.cc.lib
    pkgs.zlib
  ];
}
