{ index }:
index.lib.mkFleet {
  defaults = [ { ix.image.tag = "declared-groups"; } ];

  nodes = {
    # No fleet-level `groups`: the api image itself declares
    # `ix.networking.groups`, so its network identity travels with the
    # image definition instead of the deployment.
    api = {
      modules = [ ./api.nix ];
    };

    client = {
      dependsOn = [ "api" ];
      # Fleet-level membership still works and unions with anything the
      # image declares; this node's image is group-agnostic.
      groups = [ "declared-groups" ];
      modules = [ ./client.nix ];
    };
  };
}
