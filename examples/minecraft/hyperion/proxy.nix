# A VM players connect to. It terminates the Minecraft connection and forwards
# bytes to the game server without reading them, so this is the only part of
# the fleet with any public reachability.
{
  config,
  hyperionProxy,
  ix,
  nodes,
  ...
}: let
  # Resolved from the game server's own `ix.networking.expose` declaration, so
  # a port change there reaches here without a second edit, and a typo in the
  # listener name is an evaluation error rather than a connection refused.
  game = ix.endpointOf nodes.hyperion-game "hyperion";

  # The full name, not the bare host and not an address. The proxy uses this
  # host as the TLS server name it expects on the game server's certificate,
  # so it has to be exactly the name that certificate was issued for.
  gameFqdn = "${game.host}.ix.internal";
in {
  hyperion.pki.serverName = "${config.ix.networking.eastWest.hostName}.ix.internal";

  ix = {
    networking = {
      # `ipv4 = true` belongs here and is deliberately off, because in
      # `us-west-1` today it does not add an address, it removes the VM from
      # the network.
      #
      # The region's ingress block `15.204.22.192/26` is not delivered to the
      # OVH vRack: its gateway `15.204.22.254` is `FAILED` in the neighbour
      # table of all three hil hosts, re-checked 2026-07-29, while
      # `ip route show table 200` still reads `default via 15.204.22.254`. A
      # VM with no public address sources from `10.0.0.x`, is masqueraded, and
      # leaves over the host's own uplink. A VM that takes one sources from the
      # public block, matches the host's
      # `from 15.204.22.192/26 iif br-north lookup 200` rule, and is handed
      # that dead gateway, so everything it sends is blackholed. Both proxies
      # came up unable to resolve, unable to substitute, and unable to complete
      # their switch at all. ENG-10881.
      #
      # Still true from inside a guest, not only in the host's routing table:
      # `vip-probe`, a scratch VM holding `15.204.22.195/32`, gets
      # `Destination Host Unreachable from 10.0.0.1` pinging `1.1.1.1`, while a
      # VM with no VIP answers in 0.7 ms.
      #
      # What being off costs, stated plainly: this fleet has no public IPv4,
      # and it wants one. A Java client resolves through the JDK, which prefers
      # IPv4, and a large share of players have no IPv6 at all. Public IPv6 is
      # not a substitute either -- a guest's `/128` is reachable only from the
      # host it happens to live on (ENG-11144), so it is not ingress, it is a
      # host-local address that looks like one.
      #
      # The one public IPv4 path that does work is `services.ix.vmPublicIngress`
      # (ENG-11132): a DNAT from a *host's* own routed `bond0` address to a
      # guest address, declared in that host's inventory. It is deliberately
      # not referenced from here. It names a vmId and a guest address, so it
      # pins the VM to one host and goes stale the moment the VM moves -- that
      # is the region operator's allocation to make and to verify, not a
      # property of being a proxy. Writing it here would put a fact this fleet
      # cannot keep true into the fleet's own definition.
      #
      # Turn `ipv4` back on and recreate the proxies once OVH delivers the
      # block. The address is allocated at create; there is no
      # `ix vm set --ipv4`.

      # The one declaration of "players arrive here". Nothing outside the guest
      # reads it yet -- `ix apply` reads `groups` and `ipv4` and nothing under
      # `crates/` reads `expose` at all -- so today this opens the guest
      # firewall and nothing more.
      expose.minecraft = {
        port = 25565;
        description = "public Minecraft entrypoint";
      };

      # No name in front of that port, so a player is handed an address and
      # every correct fix to where the fleet lives costs them a new one
      # (ENG-11218; the claiming gate that has to exist first is ENG-11222).
      # The shape is decided rather than open, so it is recorded here instead
      # of being re-derived by whoever wires it up.
      #
      # One name for the fleet, declared identically by every proxy -- never
      # one name per VM. A player's endpoint has to resolve to the *role*,
      # because any proxy will do; the record set is then the union of the
      # proxies' addresses, and replacing a proxy costs one record instead of
      # the endpoint. A name per VM would publish this fleet's topology into
      # people's server lists and make every scaling decision visible to them.
      #
      # For Minecraft the record set has to include `_minecraft._tcp.<name>`
      # SRV and not only an address record, and the reason decides whether the
      # port is a scarce resource: a Java client resolves SRV before A/AAAA, so
      # the name can point at any port and the player still types only the
      # name. Two fleets can share one edge address on 25565 and 25566 with
      # neither player noticing.
      #
      # The trap in that, which is the part nobody rediscovers cheaply: after
      # SRV resolution the client puts the SRV *target* in its handshake, not
      # the name it was given. Minecraft does carry a name in its first bytes
      # -- `Server Address` in packet 0x00, which is what Velocity routes
      # virtual hosts on -- but many names pointing at one shared target all
      # arrive identical, and the demultiplexing key is gone. So either one
      # address record per fleet name, or a distinct SRV target per fleet.
      # ENG-11227.
    };

    healthChecks.hyperion-proxy.unit = "hyperion-proxy.service";
  };

  services.hyperion-proxy = {
    enable = true;
    package = hyperionProxy;
    listen = "[::]:25565";
    # A name, not an address, and that is not a stylistic choice: the host part
    # becomes the TLS server name the proxy expects on the game server's
    # certificate, which is issued for this exact string. Group members resolve
    # each other as `<name>.ix.internal` through the group's own resolver
    # (ENG-10855, fixed in ix#8978 and ix#9095), so nothing has to pin an
    # address to make this work. The `/etc/hosts` pin this file used to carry
    # for that is gone.
    gameServer = "${gameFqdn}:${toString game.port}";
    pki = {
      rootCaCert = "/var/lib/hyperion-pki/root_ca.crt";
      cert = "/var/lib/hyperion-pki/node.crt";
      privateKey = "/var/lib/hyperion-pki/node_private_key.pem";
    };
  };
}
