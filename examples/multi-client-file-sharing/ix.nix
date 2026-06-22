{ index }:

index.lib.mkFleet {
  defaults = [ { ix.image.tag = "multi-client-file-sharing"; } ];

  nodes = {
    file-server.modules = [ ./server.nix ];

    client = {
      replicas = 2;
      dependsOn = [ "file-server" ];
      modules = [ ./client.nix ];
    };
  };
}
