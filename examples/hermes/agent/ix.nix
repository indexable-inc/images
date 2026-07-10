{index}:
index.lib.mkFleet {
  nodes.hermes = {
    deployment.secrets.hermes_env = {
      file = "hermes.env";
      owner = "hermes";
      mode = "0400";
    };
    modules = [
      index.lib.hermesAgent.nixosModules.default
      index.lib.hermes.profile
    ];
  };
}
