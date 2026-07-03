{index}:
index.lib.mkFleet {
  nodes.crazy-terrain = {
    # The minecraft module declares `ix.healthChecks.minecraft`; `ix-fleet up`
    # waits for every declared check, so nothing needs selecting here.
    deployment.ipv4 = true;
    modules = [./minecraft.nix];
  };
}
