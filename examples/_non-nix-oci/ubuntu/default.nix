# OCI image on an ubuntu:24.04 base with a Nix tool layered on top.
#
# Proves the "agent lands in a normal Linux userland" path: the rootfs is
# ubuntu's own userland (so `/bin/bash`, `apt`, the FHS are all present), and a
# Nix package (`hello`) is layered on at `/bin/hello`. The base is pinned by
# digest, so the build is reproducible and never pulls a floating `24.04` tag.
{ index }:
let
  inherit (index.lib) pkgs;

  base = pkgs.dockerTools.pullImage {
    imageName = "ubuntu";
    imageDigest = "sha256:786a8b558f7be160c6c8c4a54f9a57274f3b4fb1491cf65146521ae77ff1dc54";
    hash = "sha256-bVHLrY3M5nkQP2BhFRq5wVeW/6V5t0ROMHYHYd6eWDs=";
    finalImageName = "ubuntu";
    finalImageTag = "24.04";
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
