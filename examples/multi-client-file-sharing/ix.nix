{ index }:

index.lib.mkFleet {

  nodes = {
    file-server.modules = [ ./server.nix ];

    client = {
      replicas = 2;
      dependsOn = [ "file-server" ];
      modules = [ ./client.nix ];
    };
  };
}
