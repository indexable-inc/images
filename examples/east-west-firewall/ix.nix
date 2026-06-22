{ index }:
let
  eastWestGroup = "east-west-firewall";
in
index.lib.mkFleet {
  defaults = [ { ix.image.tag = "east-west-firewall"; } ];

  nodes = {
    service = {
      groups = [ eastWestGroup ];
      modules = [ ./service.nix ];
    };

    allowed-client = {
      dependsOn = [ "service" ];
      groups = [ eastWestGroup ];
      modules = [ ./allowed-client.nix ];
    };

    outside-client = {
      dependsOn = [ "service" ];
      # No `groups`: absence from the ix group is the boundary this example checks.
      modules = [ ./outside-client.nix ];
    };
  };
}
