{index}:
index.lib.mkFleet {
  nodes.nginx = {
    deployment.recreateOnUp = true;
    modules = [./service.nix];
  };
}
