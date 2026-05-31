{
  ix,
  lib,
  pkgs,
  nodes,
  ...
}:
let
  headHost = nodes.ray-head.config.ix.networking.eastWest.hostName;
  gcsPort = 6379;
in
{
  imports = [
    (import ./cluster-node.nix {
      inherit ix lib pkgs;
      role = "worker";
      # Workers bootstrap off the head's GCS by its east-west hostname, so the
      # reference stays correct regardless of which IP the head lands on.
      extraStartArgs = [
        "--address"
        "${headHost}:${toString gcsPort}"
      ];
    })
  ];
}
