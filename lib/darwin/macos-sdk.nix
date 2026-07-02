# Pinned macOS SDK, used by `apple-sdk-toolchain.nix` to cross-compile Rust
# (and its C/C++ deps) from Linux to Darwin. Clang and zig respect `-isysroot`
# / `SDKROOT` pointing at an unpacked `MacOSX.sdk`, so a single fetched SDK is
# enough for both the C compile and the Rust link.
#
# Apple licenses the macOS SDK for use on Apple hardware. This fetches a public
# repackaged tarball; the version + URL + SRI pin live in the sibling pins.json
# (repo policy: no inline hash literals). Override `ix.macosSdk` with your own
# SDK derivation to satisfy a stricter licensing posture. The same 15.4
# tarball + hash is used by the sibling `ix` repo, so the store path is shared.
#
# Two-stage signature: lib/default.nix applies the shared `pins` reader once at
# import (a cross-directory `../util` import here is banned by no-parent-path);
# the public `ix.macosSdk` surface stays `{ pkgs }: derivation`.
{ pins }:
{ pkgs }:
let
  pin = pins.loadPin ./pins.json "macos-sdk";
  tarball = pkgs.fetchurl { inherit (pin) url hash; };
in
pkgs.runCommand "MacOSX${pin.version}.sdk" { } ''
  mkdir -p "$out"
  tar xf ${tarball} --strip-components=1 -C "$out"
''
