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

## State of the deployment, 2026-07-27

The three VMs exist in `us-west-1`. The two proxies hold public addresses,
`15.204.22.195` and `15.204.22.196`. The game server runs; the fleet as a whole
does not, and every remaining reason is a platform fault with a ticket, not
something this directory can fix. Nothing here works around one.

Fixed since this example landed:

1. **Groups can be created.** `ix group create` failed because `vm_groups` is a
   regional table with a foreign key into globally owned identity, which is a
   class two earlier sweeps had removed everywhere except here and `volumes`.
   ix#8841. `hyperion.gameAddress` was the workaround and is gone; the proxies
   resolve the game server by name again.
2. **The game server reaches `active`.** Four faults, all fixed in #4246: the
   PKI material was unreadable by the two `DynamicUser` services that need it,
   the world database opened relative to a read-only working directory, `$HOME`
   was unset so the world cache had nowhere to go, and the listen address was
   built by string concatenation.
3. **The listen address is a `SocketAddr`.** hyperion#990 gives both events one
   launcher, so `--ip ::` reaches the socket instead of panicking. The `[::]`
   workaround this file carried is deleted.

Open, and each one blocks the rest:

- **A fresh machine cannot be configured.** `nix-daemon.socket` has
  `ConditionPathIsReadWrite=/nix/var/nix/daemon-socket` and the base image does
  not create it. ENG-10487.
- **Every store path in the image belongs to nobody.** 1229 paths owned by
  `65534:65534`, so logrotate refuses its own config, its check unit fails, and
  `switch-to-configuration` exits 4 with the machine half switched. ENG-10512.
- **A guest still gets 14 GB.** The size ceiling was removed at source today
  (block volumes are provisioned lazily now), but no base image built from that
  source has been published, so an apply still fills the disk part-way through
  compiling. ENG-10522. This is a publish, not a code change.
- **An existing VM cannot join a group.** `ix group add` returns an internal
  server error and nothing logs why. ENG-10515. Until it is fixed, a VM has to
  be created with `--group`, which is how `hyperion-game` got in.

The first three are properties of one artifact. One base image built from
current source and published carries all three fixes, and that is the next
thing that has to happen for this example to run end to end.

## What this does not yet do

Two things are unverified and would show up on first apply rather than at
evaluation:

- **In-guest resolution of `*.ix.internal`.** The group DNS view exists, but
  the boot path passes `use_internal_dns: false` and guests are pointed at a
  public resolver, so whether a group member can resolve a peer's name from
  inside has not been checked here.
- **Group membership and IPv4 on a first apply.** `ix apply` on a flake target
  rejects `--group` and `--ipv4`, and does not read `ix.networking.groups` out
  of the evaluated system, so a first apply from nothing produces VMs in no
  group with no public address. Until that is fixed, create each VM once
  against an image with those flags, then converge with the command above.
