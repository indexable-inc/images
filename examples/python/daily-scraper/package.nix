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
  pname = "daily-scraper";
  version = "0.1.0";
  inherit src;
  pyChecker = "zuban";
  # pyarrow's binary wheel dlopens libstdc++ at import time.
  runtimeLibraryInputs = [ pkgs.stdenv.cc.cc.lib ];
}
