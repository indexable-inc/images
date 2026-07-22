{
  lib,
  btop,
  cmake,
  ix,
  lowdown,
  stdenv,
}: let
  # The indexable-inc/btop jj megamerge (btop-src input): upstream main plus
  # the patch DAG (macOS process disk IO sorting, kernel cwd in the process
  # detail box; see lib/fork-packages.nix). The scheduled fork-sync rebases
  # the fork repo and floats the input. Shared verbatim between the native
  # build and the cross build below, so a Mac substituting the cross output
  # runs the same patched source a native build would compile.
  patchedSrc = ix.btopSrc;

  nativeBtop = btop.overrideAttrs (old: {
    src = patchedSrc;

    meta =
      old.meta
      // {
        homepage = "https://github.com/aristocratos/btop";
      };
  });

  # Linux->Darwin cross build (RFC 0009 lane; `cross = true` in package.nix).
  # nixpkgs' own `pkgsCross.aarch64-darwin` cannot build this from Linux: the
  # Darwin stdenv bootstraps from Apple binary blobs that only execute on
  # Darwin, so the source-built SDK chain dies early (Csu-88 fails with
  # `clang: command not found`; nixpkgs#405893 tracks Linux->Darwin cross as
  # unfinished). Instead this drives btop's ordinary CMake build with the
  # cross toolchain's standalone clang + macOS SDK lane (see
  # lib/darwin/apple-sdk-toolchain.nix for why C++ executables cannot go
  # through the zig wrappers the Rust lane uses). The toolchain file pins
  # CMAKE_SYSTEM_NAME=Darwin, so btop's CMakeLists takes its APPLE branch
  # (osx sources, the CoreFoundation/IOKit frameworks, IOReport for
  # Apple-silicon GPU stats) exactly as on a native Mac build.
  crossBtop = let
    inherit (ix.cross) target;
    toolchain = ix.appleSdkToolchain {
      appleSdk = ix.macosSdk {inherit (ix) pkgs;};
      inherit lib target;
      inherit (ix) pkgs writeBashApplication;
    };
  in
    stdenv.mkDerivation {
      pname = "btop-${target}";
      inherit (btop) version;
      src = patchedSrc;

      nativeBuildInputs = [
        cmake
        # Optional in upstream's CMakeLists; present so the man page is
        # generated, matching the native nixpkgs build.
        lowdown
      ];

      cmakeFlags = [
        "-DCMAKE_TOOLCHAIN_FILE=${toolchain.standaloneCmakeToolchain}"
        # Match the native nixpkgs Darwin build: LTO breaks btop on Darwin
        # (nixpkgs#422218), and static linking is a musl/ELF affair.
        (lib.cmakeBool "BTOP_LTO" false)
        (lib.cmakeBool "BTOP_STATIC" false)
      ];

      # The output is a Mach-O arm64 binary; the Linux fixup's ELF strip
      # cannot parse it and would only warn-and-skip, so skip it explicitly.
      dontStrip = true;

      # Same meta as the native build: `platforms` already spans linux (the
      # cross build host) and darwin (the alias consumers), and `mainProgram`
      # keeps `nix run` working through the Darwin alias.
      inherit (nativeBtop) meta;
    };
in
  if ix.cross.isCross or false
  then crossBtop
  else nativeBtop
