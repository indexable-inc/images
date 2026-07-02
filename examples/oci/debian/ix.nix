# OCI image on a debian:12-slim base with a Nix tool layered on top.
#
# The debian counterpart to the ubuntu example: same builder, a different
# non-Nix base. The rootfs is debian's userland; `hello` is added at
# `/bin/hello` from a Nix layer. The base is pinned by digest for a
# reproducible, network-free-at-build image.
{ index }:
let
  inherit (index.lib) pkgs;

  # Base-image digest + fetch hash live in the sibling pins.json, never inline
  # (repo policy: examples consume pinned data, they don't own hash literals).
  pin = index.lib.pins.loadPin ./pins.json "base";

  base = pkgs.dockerTools.pullImage {
    inherit (pin) imageName imageDigest hash;
    finalImageName = pin.imageName;
    inherit (pin) finalImageTag;
  };
in
index.lib.mkNonNixImage {
  name = "ix/debian-base";
  tag = "12-slim";
  baseImage = base;
  contents = [ pkgs.hello ];
  config = {
    Cmd = [ "/bin/bash" ];
  };
}
