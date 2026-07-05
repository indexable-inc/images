{
  ix,
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
  fontconfig,
  freetype,
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

    # font-kit (pulled in through nu_plugin_plotters) compiles
    # yeslogic-fontconfig-sys and freetype-sys on Linux only (macOS uses
    # core-text instead). Their build scripts locate the system libraries via
    # pkg-config; without these inputs the fontconfig probe fails and the
    # package broke every x86_64-linux Cache push run
    # (indexable-inc/index#1863).
    nativeBuildInputs = lib.optional stdenv.hostPlatform.isLinux pkg-config;
    buildInputs = lib.optionals stdenv.hostPlatform.isLinux [
      fontconfig
      freetype
    ];

    meta = {
      description = "A Jupyter raw kernel for Nushell";
      homepage = "https://github.com/cptpiepmatz/nu-jupyter-kernel";
      license = lib.licenses.mit;
      mainProgram = "nu-jupyter-kernel";
    };
  }
