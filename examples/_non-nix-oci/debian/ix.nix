# OCI image on a debian:12-slim base with a Nix tool layered on top.
#
# The debian counterpart to the ubuntu example: same builder, a different
# non-Nix base. The rootfs is debian's userland; `hello` is added at
# `/bin/hello` from a Nix layer. The base is pinned by digest for a
# reproducible, network-free-at-build image.
{ index }:
let
  inherit (index.lib) pkgs;

  base = pkgs.dockerTools.pullImage {
    imageName = "debian";
    imageDigest = "sha256:0104b334637a5f19aa9c983a91b54c89887c0984081f2068983107a6f6c21eeb";
    hash = "sha256-7BIcwvTwDJeqbKT6wNQ86l4O936LjmrnEgzZesSHGuc=";
    finalImageName = "debian";
    finalImageTag = "12-slim";
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
