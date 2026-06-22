{ index }:

index.lib.mkFleet {
  # The tag is shared by every replacement image this example builds, so
  # registry destinations read `factions:factions` instead of `:latest`.
  defaults = [ { ix.image.tag = "factions"; } ];

  nodes.factions = {
    deployment.ipv4 = true;
    modules = [ ./minecraft.nix ];
  };
}
