# Pinned macOS SDK, used by `apple-sdk-toolchain.nix` to cross-compile Rust
# (and its C/C++ deps) from Linux to Darwin. Clang and zig respect `-isysroot`
# / `SDKROOT` pointing at an unpacked `MacOSX.sdk`, so a single fetched SDK is
# enough for both the C compile and the Rust link.
#
# Apple licenses the macOS SDK for use on Apple hardware. This fetches a public
# repackaged tarball and pins it by SRI hash; override `ix.macosSdk` with your
# own SDK derivation to satisfy a stricter licensing posture. The same 15.4
# tarball + hash is used by the sibling `ix` repo, so the store path is shared.
{ pkgs }:
let
  tarball = pkgs.fetchurl {
    url = "https://github.com/joseluisq/macosx-sdks/releases/download/15.4/MacOSX15.4.sdk.tar.xz";
    hash = "sha256-oLe2aRKsDaDkWzBKMyus2+WMoXIiCCDUJe2yghOWL4E=";
  };
in
pkgs.runCommand "MacOSX15.4.sdk" { } ''
  mkdir -p "$out"
  tar xf ${tarball} --strip-components=1 -C "$out"
''
