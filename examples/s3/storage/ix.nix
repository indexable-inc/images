{index}:
index.lib.mkFleet {
  # No `recreateOnUp`: this node holds object data, so it persists across
  # `ix-fleet up` instead of being rebuilt each time like the nginx demo.
  nodes.s3 = {
    modules = [./service.nix];
  };
}
