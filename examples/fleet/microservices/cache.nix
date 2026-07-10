{...}: let
  redisPort = 6379;
in {
  # The east-west group is a private network between this fleet's nodes, so
  # the demo cache runs unauthenticated; bind beyond loopback so api replicas
  # can reach it by node name.
  services.redis.servers.cache = {
    enable = true;
    port = redisPort;
    bind = "0.0.0.0";
  };

  # One declaration opens the firewall, registers the port claim, and lets
  # api replicas resolve this listener with `ix.endpointOf nodes.cache "redis"`.
  ix.networking.expose.redis = {
    port = redisPort;
    description = "shared cache for the api replicas";
  };

  ix.healthChecks = {
    redis.unit = "redis-cache";

    # A tcpSocket-style probe: healthy once the port accepts a connection.
    # The platform derives the `nc -z` command and keeps the probe binary
    # in the image closure.
    accepting-connections = {
      description = "cache accepts TCP connections";
      tcp.port = redisPort;
    };
  };
}
