/**
  A small, stringifiable network endpoint value shared across the fleet.

  `endpoint { host; port; }` returns an attrset that renders to `host:port` in
  string context (via `__toString`) yet still exposes `.host`, `.port`,
  `.authority`, and `.url` for callers that need a part. This kills the
  `"${host}:${toString port}"` boilerplate and the stray `toString port` that
  litter cross-node wiring, and gives one type every producer/consumer agrees on.

  `endpointOf node "name"` resolves a *peer's* declared listener: it reads that
  node's `ix.networking.expose.<name>` port and pairs it with the node's
  east-west hostname, so a consumer never reaches into a sibling's internal
  option tree to discover where to connect.

      kafka = ix.endpointOf nodes.log "kafka";   # => log:9092
      "--bootstrap-server" "${kafka}"            # renders host:port
      kafka.port                                 # => 9092

  For a listener that is not declared through `expose` (an upstream module's own
  option), build one directly: `ix.endpoint { host = peerHost; port = thePort; }`.
*/
{ lib }:
let
  endpoint =
    {
      host,
      port,
      # Optional application scheme (http, grpc, ...). When set, `url` becomes
      # `scheme://host:port<path>`; when null, `url` is the bare `host:port`
      # authority, which is what most callers interpolate.
      scheme ? null,
      path ? "",
    }:
    let
      authority = "${host}:${toString port}";
    in
    {
      inherit
        host
        port
        scheme
        path
        authority
        ;
      url = if scheme == null then authority else "${scheme}://${authority}${path}";
      __toString = self: self.url;
    };

  endpointOf =
    node: name:
    let
      config = node.config or node;
      listeners = config.ix.networking.expose;
    in
    assert lib.assertMsg (listeners ? ${name})
      "endpointOf: node '${
        config.networking.hostName or "?"
      }' has no `ix.networking.expose.${name}` listener; declare it with `ix.networking.expose.${name}` on that node, or build an endpoint directly with `ix.endpoint`.";
    endpoint {
      host = config.ix.networking.eastWest.hostName;
      inherit (listeners.${name}) port;
    };
in
{
  inherit endpoint endpointOf;
}
