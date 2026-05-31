{ index }:

index.lib.mkFleet {
  defaults = [ { ix.image.tag = "s3-storage"; } ];

  # No `recreateOnUp`: this node holds object data, so it persists across
  # `ix-fleet up` instead of being rebuilt each time like the nginx demo.
  nodes.s3 = {
    modules = [ ./service.nix ];
  };
}
