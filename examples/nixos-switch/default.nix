{ index }:

# A one-node NixOS fleet that exists to demonstrate the native `ix switch` loop:
# `ix up` builds this configuration on ix and activates it on the running VM in
# place. Edit `configuration.nix`, run it again, and the VM converges.
index.lib.mkFleet {
  defaults = [ { ix.image.tag = "nixos-switch"; } ];

  nodes.devbox = {
    modules = [ ./configuration.nix ];
  };
}
