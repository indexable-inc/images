{ index }:

index.lib.mkFleet {
  defaults = [ { ix.image.tag = "hermes-agent"; } ];

  nodes.hermes = {
    modules = [
      index.lib.hermesAgent.nixosModules.default
      ./hermes.nix
    ];
  };
}
