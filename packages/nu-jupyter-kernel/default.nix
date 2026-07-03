{
  ix,
  lib,
  rustPlatform,
}: let
  src = ix.nuJupyterKernelSrc;
in
  rustPlatform.buildRustPackage {
    pname = "nu-jupyter-kernel";
    version = "0.1.14";
    inherit src;
    __structuredAttrs = true;
    strictDeps = true;

    cargoLock.lockFile = src + "/Cargo.lock";

    meta = {
      description = "A Jupyter raw kernel for Nushell";
      homepage = "https://github.com/cptpiepmatz/nu-jupyter-kernel";
      license = lib.licenses.mit;
      mainProgram = "nu-jupyter-kernel";
    };
  }
