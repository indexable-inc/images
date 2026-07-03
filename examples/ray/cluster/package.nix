{
  ix,
  lib,
  pkgs ? ix.pkgs,
}: let
  fs = lib.fileset;
  src = fs.toSource {
    root = ./.;
    fileset = fs.unions [
      ./pyproject.toml
      ./src
      ./uv.lock
    ];
  };
  loader = import ./loader.nix {inherit lib pkgs;};
in
  ix.buildUvApplication pkgs {
    pname = "ray-demo";
    version = "0.1.0";
    inherit src;
    pyChecker = "zuban";
    # Ray's wheel bundles a compiled `_raylet.so` that the Python driver dlopens
    # at import; it needs a modern libstdc++ and zlib on the loader path. The
    # standalone `raylet`/`gcs_server` daemons that `ray start` execs are FHS
    # binaries served by the image's `nix-ld`; cluster-node.nix points `NIX_LD*`
    # at that same set. See loader.nix.
    runtimeLibraryInputs = loader.libs;
  }
