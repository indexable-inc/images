{
  lib,
  stdenv,
  rustPlatform,
  pkg-config,
  libssh2,
  openssl,
  zlib,
  python3,
}:
# Build the PyO3 cdylib (standalone crate, vendored Cargo.lock) and package it
# plus the Python source into an abi3 wheel with wheel/mkwheel.py. ssh2 links the
# system libssh2/openssl from nix (no vendored C build): OPENSSL_NO_VENDOR makes
# openssl-sys use the nix openssl, and LIBSSH2_SYS_USE_PKG_CONFIG makes
# libssh2-sys link the nix libssh2 via pkg-config instead of compiling its own.
# The wheel references those store paths via rpath, so it runs inside this nix env
# (it is not a portable manylinux wheel).
let
  pyproject = lib.importTOML ./pyproject.toml;
  inherit (pyproject.project) version;

  platformTag =
    {
      aarch64-darwin = "macosx_11_0_arm64";
      x86_64-darwin = "macosx_10_12_x86_64";
      x86_64-linux = "manylinux_2_34_x86_64";
      aarch64-linux = "manylinux_2_34_aarch64";
    }
    .${stdenv.hostPlatform.system}
      or (throw "polars-sftp: unsupported system ${stdenv.hostPlatform.system}");
in
rustPlatform.buildRustPackage {
  pname = "polars-sftp";
  inherit version;

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./build.rs
      ./src
      ./python
      ./pyproject.toml
      ./wheel
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  strictDeps = true;
  nativeBuildInputs = [
    pkg-config
    python3
  ];
  buildInputs = [
    libssh2
    openssl
    zlib
  ];

  env.OPENSSL_NO_VENDOR = "1";
  env.LIBSSH2_SYS_USE_PKG_CONFIG = "1";

  # The default install step runs `cargo install`, which has nothing to install
  # for a cdylib-only crate. Replace it: find the cdylib the build hook produced
  # and package the wheel.
  installPhase = ''
    runHook preInstall
    # Match the cdylib in the target's release dir, not the identical copy under
    # release/deps/, so the result is deterministic.
    cdylib=$(find target \( -path '*/release/libpolars_sftp.so' -o -path '*/release/libpolars_sftp.dylib' \) | head -1)
    if [ -z "$cdylib" ]; then
      echo "polars-sftp: cdylib not found under target/" >&2
      find target -name 'libpolars_sftp.*' >&2 || true
      exit 1
    fi
    mkdir -p "$out"
    python3 ${./wheel/mkwheel.py} \
      --cdylib "$cdylib" \
      --python-src ${./python} \
      --version ${version} \
      --platform-tag ${platformTag} \
      --out "$out"
    runHook postInstall
  '';

  # The crate has no Rust tests; validation is the Python end-to-end (SFTP scan).
  doCheck = false;

  meta = {
    description = "Polars IO source for remote files over SFTP (scan_sftp), as an abi3 wheel";
    license = lib.licenses.mit;
    platforms = [
      "aarch64-darwin"
      "x86_64-darwin"
      "x86_64-linux"
      "aarch64-linux"
    ];
  };
}
