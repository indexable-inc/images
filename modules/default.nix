# Module registry. Single source of truth for both:
#   - `nixosSystem { modules = ... }` (leaf paths collected by lib/)
#   - the `nixosModules` flake output (consumed by external users who want one
#     module without depending on the rest of the ix base layer)
#
# Every module is gated on its own `enable` flag. Listing it here only makes
# the options visible; it does not turn anything on. The one exception is
# `profiles/base/`, which `lib/ix-oci-layer.nix` enables by default.
{
  base = ./profiles/base;
  extended-attributes = ./profiles/extended-attributes.nix;
  jvm = ./profiles/jvm.nix;
  floodgate = ./services/floodgate.nix;
  geyser = ./services/geyser.nix;
  git-clone = ./services/git-clone.nix;
  minestom = ./services/minestom.nix;
  minecraft-bedrock = ./services/minecraft-bedrock.nix;
  postgresql = ./services/postgresql.nix;
  observability = ./services/observability;
  remote-desktop = ./services/remote-desktop.nix;
  resource-monitor = ./services/resource-monitor;
  velocity = ./services/velocity.nix;

  minecraft = {
    runtime = ./services/minecraft;
    fabric = ./services/minecraft/fabric.nix;
    folia = ./services/minecraft/folia.nix;
    neoforge = ./services/minecraft/neoforge.nix;
    paper = ./services/minecraft/paper.nix;
    purpur = ./services/minecraft/purpur.nix;
    spigot = ./services/minecraft/spigot.nix;
    sponge = ./services/minecraft/sponge.nix;
    vanilla = ./services/minecraft/vanilla.nix;
    mods = {
      bluemap = ./services/minecraft/mods/bluemap.nix;
      distant-horizons = ./services/minecraft/mods/distant-horizons.nix;
      luckperms = ./services/minecraft/mods/luckperms.nix;
      simple-voice-chat = ./services/minecraft/mods/simple-voice-chat.nix;
      terraformgenerator = ./services/minecraft/mods/terraformgenerator.nix;
    };
  };
}
