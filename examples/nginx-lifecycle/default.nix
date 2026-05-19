{ index }:

index.lib.mkFleet {
  defaults = [ { ix.image.tag = "nginx-lifecycle"; } ];

  nodes.nginx = {
    deployment.recreateOnUp = true;
    modules = [ ./service.nix ];
  };
}
