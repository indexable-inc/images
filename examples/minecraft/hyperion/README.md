# hyperion: one game server, three proxies

```
players -> hyperion-proxy-0 \
players -> hyperion-proxy-1  >-- hyperion-game (private, no public address)
players -> hyperion-proxy-2 /
```

```sh
nix build .#hyperion-game-system .#hyperion-proxy-0-system .#hyperion-proxy-1-system .#hyperion-proxy-2-system
ix apply .#hyperion-game .#hyperion-proxy-0 .#hyperion-proxy-1 .#hyperion-proxy-2
```

Change anything and run the same command again. Each VM is reused by name and
switched in place, so only the units whose definition changed restart.

## Build the fleet once, not once per VM

The `nix build` line is what makes the apply finish in minutes instead of
hours, and it is the only reason for the `packages` output in `flake.nix`.

Without it, `ix apply` hands each VM the derivation and each VM realises it in
its own store. The nodes here are one closure with several hostnames; measured
over the three-node fleet:

    union of the three systems   1766 store paths, 7.51 GB
    any one of them              7.50 GB

So three guests compile the same 7.5 GB three times, from a store that starts
with only what the base image carries, over a link to two public caches that do
not serve a Rust workspace from someone else's flake. That is the 83-minute
apply that never finished (ENG-10800, ENG-10839).

Build it first and the numbers are different, because the work happens once, on
a machine that is a builder:

    nix build, all three nodes    195s (78 derivations, one remote builder)
    VM create, per node           1.0s to 1.7s, "reusing golden snapshot"

`ix apply` then finds the system already in your store and exports the closure
instead of asking the guest to produce it (`SwitchTarget::LocalClosure`): the
guest answers with the store paths it lacks and only those cross. No guest
compiles anything.

Where `nix build` runs is your choice and nothing about ix makes it: it is your
nix, your `/etc/nix/machines`. On a Mac it has to be a remote builder, because
these are `x86_64-linux` systems. `ix apply` itself still never builds and
never substitutes; if the paths are not already there, it behaves exactly as
before.

What this does not fix: the closure crosses your uplink once per VM. Nodes
sharing one closure still pay one export each, so the third proxy costs another
one. The shape that avoids that is one push into the region's CAS and N VMs
materializing from it, which is what ENG-10839 is about.

## Why the game server has no public address

Proxies dial the game server, not the other way round. So the game server
needs one address its proxies can reach and nothing else, and it lives only on
the `hyperion` east-west group. A VM outside that group has no route to it,
which is the only thing keeping an unproxied client off the world.

The proxy count is one digit in `default.ix`: `replicas: 3`. `mkFleet` expands
that into `hyperion-proxy-0` through `hyperion-proxy-2`, which is why the
proxies are one spec rather than three copy-pasted node entries -- with three
entries their interchangeability is a comment, and one of them can be edited
alone. The game server does not change when the count does: it has supported
several connected proxies since hyperion#940.

## Why the proxy names the game server rather than addressing it

`--server` takes a `host:port`, and the host becomes the TLS server name the
proxy expects on the game server's certificate. An address there fails the
handshake against a certificate issued for a name, and the failure reads as a
connection problem rather than a naming one. Group members resolve each other
as `<name>.ix.internal`, so that is the string on both sides.

That resolution now works, which is new. This file used to document an
`/etc/hosts` pin (`hyperion.gameAddress`) that mapped the name to an address to
keep the name while skipping the resolver. ix#8978 gives each group a
`<prefix>::1` gateway and ix#9095 makes ix-dns bind an IPv6 listener on it, so
a member's query carries a source address the resolver classifies into its own
group's view (ENG-10855). Measured inside `hyperion-proxy-0`:

    $ head -1 /etc/resolv.conf
    nameserver fd00:1:1f:856f::1
    $ getent hosts hyperion-game.ix.internal
    fd00:1:1f:856f:4250:a3a5:16b7:d9ef hyperion-game.ix.internal

The option and the `networking.hosts` block it fed are deleted.

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

## Nothing here can spread the proxies across hosts

Three proxies are three failure domains for the proxy process and one for the
hardware under them, and this directory cannot change that. Measured:

    $ ix ls --output json | jq -r '.[] | "\(.name)  \(.node)"'
    hyperion-proxy-1  hil-compute-1
    hyperion-proxy-0  hil-compute-1
    hyperion-game     hil-compute-3

Both proxies on one host. That is not bad luck. `select_by_available_memory`
orders candidate hosts by the `available_memory_mib` their last heartbeat
reported and reserves nothing against a placement it has just made, so every VM
in one `ix apply` goes to whichever host was ahead at that instant. There is no
anti-affinity concept to declare and no host to name -- `ix apply` has no flag,
and the fleet node spec has no key. ENG-11225.

So read the redundancy claim narrowly: a proxy crashing, or being switched to a
new system, leaves the others serving. A host failing may take all of them. The
honest fix is a spread key the scheduler honours; a `--node` flag would only
move the problem into whoever types the command.

## There is no name in front of the endpoint

A player is handed an address, so every correct fix to where this fleet lives
costs them a new one. ENG-11218 carries that; the claiming gate that has to
exist before any name can be handed out is ENG-11222.

The shape the fleet should declare is written at length beside
`expose.minecraft` in `proxy.nix`, because it is decided rather than open. The
two load-bearing parts:

- **One name for the fleet, on the proxy role, never one per VM.** The player's
  endpoint must resolve to "a proxy", so the record set is the union of the
  proxies' addresses and replacing a proxy costs a record instead of an
  endpoint.
- **`_minecraft._tcp.<name>` SRV, not an address record alone.** A Java client
  resolves SRV first, so the name can point at any port while the player still
  types only the name -- which means a public Minecraft port is not the scarce
  per-host resource it is usually treated as. The catch is that after SRV
  resolution the client sends the SRV *target* in its handshake, so several
  names behind one shared target arrive indistinguishable and name-based
  demultiplexing at an edge stops working. ENG-11227.

## State of the deployment, 2026-07-29

Three VMs exist in `us-west-1`: `hyperion-game` on hil-compute-3,
`hyperion-proxy-0` and `hyperion-proxy-1` on hil-compute-1. The third proxy is
in this spec and evaluates; **it has not been created.**

**The fleet serves Minecraft, across hosts, through either proxy.** A real
status handshake -- protocol 776, the same packet a client sends -- against
each proxy returns the game server's own status:

    $ ix shell hyperion-game -- sh -c 'python3 /tmp/mcping.py \
        hyperion-proxy-0.ix.internal 25565'
    version:     {'name': '26.2', 'protocol': 776}
    players:     {'max': 12000, 'online': 0, 'sample': []}
    description: Getting 10k Players to PvP at Once on a Minecraft Server to
                 Break the Guinness World Record

Identical through `hyperion-proxy-1`. The path crosses hosts twice --
hil-compute-3 to hil-compute-1 to reach the proxy, hil-compute-1 back to
hil-compute-3 for the game server -- so this exercises the whole design: group
name resolution, the VXLAN data path, and the proxy's mutual-TLS link to the
game server.

`mcping.py` is in this directory. Run it from inside the group, where the names
resolve; it speaks the protocol rather than asking systemd for an opinion, which
is the difference that matters (see below).

The same test doubles as a check that the game server is not directly playable,
which is the property the private segment and the client certificate exist for.
Pointed at `hyperion-game:35565` it fails, and the raw bytes say why -- a
plaintext handshake gets exactly seven back:

    15 0303 0002 02 32

A TLS record of type 21, alert, fatal, `decode_error`. The game port speaks TLS
and refuses anything else, so reaching that port is not the same as being able
to use it. Through the script the same thing reads as
`expected status response (packet 0), got packet 3`.

**What is missing is public ingress, and only that.** No player can reach a
proxy from the internet: the proxies hold no public IPv4 (see below), and a
guest's public IPv6 `/128` answers only from the host it lives on (ENG-11144).
Every hop from a player's client to the world exists and is tested except the
first one.

Two things this run corrected that were true this morning:

- **Cross-host east-west works now.** It did not. `hyperion-proxy-0` could
  reach `hyperion-proxy-1` on the same host in 0.24 ms and got
  `Address unreachable` for the game server one host over. ix#9073
  (ENG-10976, ENG-11067) landed and the same ping is now 3/3 at 0.33 ms.
- **A `systemctl` reading of `active` proved nothing while it was broken.**
  hyperion-proxy binds its listener at startup and dials the game server per
  connection, so a completely unreachable backend was invisible to systemd.
  The unit crash-looped when the *name* would not resolve and went quiet when
  only the *path* was broken, which is the wrong way round. The handshake above
  is the check that would have caught it; nothing in the repo runs it
  (ENG-10986).

Fixed since this example landed:

1. **Groups can be created, a first apply joins them, and members resolve each
   other.** `ix group create` failed because `vm_groups` is a regional table
   with a foreign key into globally owned identity (ix#8841). `ix apply` reads
   `ix.networking.groups` off the evaluated system, so the group and its
   membership come up on a first apply from nothing. Resolution inside the group
   is ENG-10855, closed by ix#8978 and ix#9095 (see above).
2. **The game server reaches `active`.** Four faults, all fixed in #4246: the
   PKI material was unreadable by the two `DynamicUser` services that need it,
   the world database opened relative to a read-only working directory, `$HOME`
   was unset so the world cache had nowhere to go, and the listen address was
   built by string concatenation.
3. **The listen address is a `SocketAddr`.** hyperion#990 gives both events one
   launcher, so `--ip ::` reaches the socket instead of panicking.
4. **A guest no longer has to build.** With the systems built first, each switch
   goes straight to `importing closure`; nothing compiles in a VM. The three
   faults this file used to list as "properties of one artifact" (ENG-10487
   `nix-daemon.socket`, ENG-10512 store paths owned by nobody, ENG-10522 the
   14 GB root) were all about a guest trying to build. None of them appeared.

Open:

- **Public ingress. This is the only thing left between the fleet and a
  player.** The region's one Additional IP block, `15.204.22.192/26`, is
  delivered nowhere: OVH reports `routedTo.serviceName = null` for it. The fix
  is attaching it to the vRack, and that is refused because all three vRacks
  report `resource.state = "suspended"` and every vRack call answers HTTP 460,
  "This service is expired" -- with no billing cause. OVH ticket 713661,
  ENG-11229, ENG-10881. Until then, taking an address from the block is worse
  than having none: `vip-probe`, a scratch VM holding `15.204.22.195/32`, gets
  `Destination Host Unreachable from 10.0.0.1` pinging `1.1.1.1`, while a VM
  with no VIP answers in 0.7 ms.

  The path that works needs no provider action: a DNAT from a hil host's own
  routed `bond0` address to the proxy on it, via `services.ix.vmPublicIngress`
  (ENG-11132). Host addresses route -- `15.204.109.254` answers a laptop in
  21 ms. `proxy.nix` explains why that is the right shape here rather than only
  the available one, and what it costs.

  One warning if you go looking: **no ARP probe can tell delivered from
  undelivered.** OVH answers ARP for every address on that segment, including
  `198.51.100.7`, which is TEST-NET-2 and belongs to nobody here. A check built
  on the gateway answering cannot fail.
- **The name resolves to the wrong region.** ENG-11218, gated on ENG-11222.
  Not merely missing: `*.ix.dev` is an apex wildcard pointing at
  `40.160.30.136`, a VIN host, and asked of Cloudflare's own nameservers it
  answers for `hyperion.apps.ix.dev`, `mc.apps.ix.dev`, `play.ix.dev`, and also
  for `hil-compute-1.host.ix.dev` and `hil-compute-3.host.ix.dev` -- the hosts
  that would carry the proxies. None has an A record of its own, none has any
  AAAA. So a client told any of these opens a connection to the wrong region
  and hangs. Explicit records win over the wildcard and are generated from
  inventory in ix's `nix/terraform/cloudflare/dns-ix-dev.nix`; shape in
  `proxy.nix`.
- **A group's DNS gateway is not consistently present.** ENG-11226. On the
  `hyperion` group's bridge, `fd00:1:1f:856f::1` is on hil-compute-1 and
  hil-compute-3 and absent on hil-compute-2, which carries two members of the
  same group. It has stopped being latent now that cross-host traffic works: a
  member placed on hil-compute-2 has no in-prefix resolver on its own host.
- **The proxies may all be on one host and nothing can ask otherwise.**
  ENG-11225, above.
- **A fleet cannot boot its own published image.** ENG-10839. `mkFleet` renders
  one per node at `packages.<node>`, and nothing outside the private ix repo can
  build it, which is why the export cost above is per-VM rather than a single
  push into the region's CAS.
- **Nothing in CI runs either check in this directory.** ENG-10986:
  `exampleFleetsFor` skips this example because it depends on an external flake,
  so both the eval below and `mcping.py` are commands someone has to remember
  rather than gates. `mcping.py` is the one that would have caught the twelve
  hours of `active` above.

## Evaluating it without deploying

Every node's closure, from this directory, against the index checkout you are
editing:

```sh
nix eval --override-input index /path/to/index \
  .#nixosConfigurations \
  --apply 'cs: builtins.mapAttrs (n: c: c.config.system.build.toplevel.drvPath) cs'
```

11 seconds, and it forces all four systems, so a missing attribute, a bad option
or a port collision fails immediately. It builds nothing and deploys nothing: an
eval passing says the fleet is well formed, not that it works.

Two things about that command that cost time to find. Write the override as a
plain path, **not** `path:/path/to/index`: the `path:` fetcher copies the tree
into the store and `lib.fileset.gitTracked` then refuses it ("The argument is a
store path within a working tree of a Git repository"). And the plain path is a
git flakeref, so it reads `HEAD`, not your working tree -- commit before you
evaluate or you will confidently evaluate the previous version.

Holding the index input constant also makes the eval a diff tool. Evaluating
this directory's current spec against the index revision that predates it
reproduces the previous fleet's derivation paths exactly:

    hyperion-game     k4nxy1l152lq4vq8wfv7q5d926nxqzl8   unchanged
    hyperion-proxy-0  aragzfail81z0fdz38dzgzm126jqvrjj   unchanged
    hyperion-proxy-1  s1k599cw6blsh9927f3cv8l0bh7f7y4v   unchanged
    hyperion-proxy-2  8ir5pdfimwh89c375cq8hml44yxqpk7n   new

So folding the two copy-pasted proxy nodes into `replicas: 3` and deleting the
resolved DNS workaround changed no existing node's system at all -- re-applying
switches and restarts nothing, and the only new closure is the third proxy.
Worth doing before any apply that claims to be a no-op.

The same trick checks that the replicas really are interchangeable rather than
merely evaluating. `nix derivation show` on two of them differs in 2 of 17 input
derivations, `etc` and `activate`, and in exactly two attributes, `name` and the
`buildCommand` that embeds the `etc` path. `system-path` and every package under
it are the same store path. The only thing separating one proxy from the next is
its hostname, which is what "interchangeable" has to mean to be worth saying.
