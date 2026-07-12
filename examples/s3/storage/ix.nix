{index}:
index.lib.mkFleet {
  # This node holds object data, so repeated `ix up` runs reconcile it in place.
  nodes.s3 = {
    modules = [./service.nix];
  };
}
