{ index }:

index.lib.mkFleet {

  nodes.workbench = {
    modules = [ ./tools.nix ];
  };
}
