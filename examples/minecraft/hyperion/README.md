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

## State of the deployment, 2026-07-27

The three VMs exist in `us-west-1`. The two proxies hold public addresses,
`15.204.22.195` and `15.204.22.196`. Neither runs hyperion yet, and the reason
is the platform rather than anything here. Three faults, in the order they
were hit:

1. **Groups cannot be created.** `ix group create hyperion` returns an
   internal server error, and so does `ix apply <image> --group hyperion`. The
   same create without the flag works in under two seconds. ENG-10486. The
   whole shape depends on this: the game server has no public address, so the
   group is the only route to it. `hyperion.gameAddress` exists as the
   workaround and should go back to null the day groups work.
2. **A fresh VM cannot be switched.** Every apply ended on `nix daemon socket
   did not become ready`, which reads like a timeout. The cause is a missing
   directory: `nix-daemon.socket` has
   `ConditionPathIsReadWrite=/nix/var/nix/daemon-socket` and the base image
   does not create it. ENG-10487. Creating it by hand got the next apply
   further.
3. **The guest cannot build this.** `ix apply` compiles the closure inside the
   VM, from a cache holding the public world but nothing built privately, and
   a guest gets a 14 GB root with no way to ask for more. A Rust game server
   does not fit: the disk filled twice, once surfacing as a truncated download
   and once as `No space left on device`. The same closure builds in minutes
   on the Linux remote builder. ENG-10488.

The topology, the certificates and the units are written and evaluate. What is
missing is a way to get a large closure onto a machine.
