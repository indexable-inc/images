# hyperion: one game server, two proxies

```
players -> hyperion-proxy-0 \
                             >-- hyperion-game (private, no public address)
players -> hyperion-proxy-1 /
```

```sh
ix apply .#hyperion-game .#hyperion-proxy-0 .#hyperion-proxy-1
```

Change anything and run the same command again. Each VM is reused by name and
switched in place, so only the units whose definition changed restart.

## Why the game server has no public address

Proxies dial the game server, not the other way round. So the game server
needs one address its proxies can reach and nothing else, and it lives only on
the `hyperion` east-west group. A VM outside that group has no route to it,
which is the only thing keeping an unproxied client off the world.

Adding a third proxy is one more line in `default.ix`. The game server does
not change: it has supported several connected proxies since hyperion#940.

## Why the proxy names the game server rather than addressing it

`--server` takes a `host:port`, and the host becomes the TLS server name the
proxy expects on the game server's certificate. An address there fails the
handshake against a certificate issued for a name, and the failure reads as a
connection problem rather than a naming one. Group members resolve each other
as `<name>.ix.internal`, so that is the string on both sides.

## The certificate authority in this directory is public

`dev-ca.key` is committed. Every node installs it, mints its own leaf, and
deletes it. That is what makes the example run with no secret plumbing, and it
means anything using it is a demonstration rather than a deployment: the key
is in this repository and in the nix store, and anyone holding it can
impersonate a proxy and take commands to your world.

For anything real, deliver a key out of band and point `hyperion.pki.caKeyFile`
at it.

One shared secret rather than three certificates: every node gets the same CA
key and mints its own leaf at activation, so nothing has to copy a certificate
from one VM to another.

## What this does not yet do

Two things are unverified and would show up on first apply rather than at
evaluation:

- **In-guest resolution of `*.ix.internal`.** The group DNS view exists, but
  the boot path passes `use_internal_dns: false` and guests are pointed at a
  public resolver, so whether a group member can resolve a peer's name from
  inside has not been checked here. If it cannot, the fix is a
  `networking.hosts` entry per peer, which needs a group address available at
  evaluation time and `ix.networking.eastWest` exposes only a name.
- **Group membership and IPv4 on a first apply.** `ix apply` on a flake target
  rejects `--group` and `--ipv4`, and does not read `ix.networking.groups` out
  of the evaluated system, so a first apply from nothing produces VMs in no
  group with no public address. Until that is fixed, create each VM once
  against an image with those flags, then converge with the command above.
