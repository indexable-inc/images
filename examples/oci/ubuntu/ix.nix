# OCI image on an ubuntu:24.04 base with a Nix tool layered on top.
#
# Proves the "agent lands in a normal Linux userland" path: the rootfs is
# ubuntu's own userland (so `/bin/bash`, `apt`, the FHS are all present), and a
# Nix package (`hello`) is layered on at `/bin/hello`. The base is pinned by
# digest, so the build is reproducible and never pulls a floating `24.04` tag.
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
  name = "ix/ubuntu-base";
  tag = "24.04";
  baseImage = base;
  contents = [ pkgs.hello ];
  config = {
    # Bash comes from the ubuntu base; `hello` from the Nix layer is on PATH
    # because the base sets PATH to include /bin.
    Cmd = [ "/bin/bash" ];
  };
}
