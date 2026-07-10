{index}:
index.lib.mkFleet {
  nodes.factions = {
    deployment.ipv4 = true;
    modules = [./minecraft.nix];
  };
}
