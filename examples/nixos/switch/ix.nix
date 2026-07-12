{index}:
# A one-node NixOS fleet consumed by flake.nix for the native `ix up` loop:
# `ix up .#devbox` builds this configuration on ix and activates it on the
# running VM in place. Edit `configuration.nix`, run it again, and the VM
# converges.
index.lib.mkFleet {
  nodes.devbox = {
    modules = [./configuration.nix];
  };
}
