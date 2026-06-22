{ index }:

index.lib.mkFleet {
  defaults = [ { ix.image.tag = "polyglot-dev"; } ];

  nodes.workbench = {
    modules = [ ./tools.nix ];
  };
}
