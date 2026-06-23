{ index }:

index.lib.mkFleet {

  nodes.hermes = {
    modules = [
      index.lib.hermesAgent.nixosModules.default
      index.lib.hermes.profile
    ];
  };
}
