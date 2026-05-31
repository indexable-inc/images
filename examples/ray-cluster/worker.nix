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
  rayAddress = "${headHost}:${toString gcsPort}";
in
{
  imports = [
    (import ./cluster-node.nix {
      inherit
        ix
        lib
        pkgs
        rayAddress
        ;
      role = "worker";
      # Workers bootstrap off the head's GCS by its east-west hostname, so the
      # reference stays correct regardless of which IP the head lands on.
      extraStartArgs = [
        "--address"
        rayAddress
      ];
    })
  ];
}
