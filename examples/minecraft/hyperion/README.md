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

Three VMs exist in `us-west-1`: `hyperion-game`, `hyperion-proxy-0`,
`hyperion-proxy-1`. The third proxy is in this spec and evaluates; **it has not
been created.** Nothing below was verified by applying it.

What works now that did not before: both proxies resolve the game server and
`hyperion-proxy.service` has stayed `active` for 12 hours instead of
crash-looping. Full-fleet play does not work, and the reason is one layer under
DNS.

**Names resolve; packets do not cross hosts.** From `hyperion-proxy-0`
(hil-compute-1), against `hyperion-game` (hil-compute-3):

    ping hyperion-proxy-1.ix.internal   (same host)   2/2 received, 0.24 ms
    ping hyperion-game.ix.internal      (other host)  0/2, Address unreachable
    connect hyperion-game.ix.internal:35565            No route to host

Same-host east-west works, cross-host east-west does not. That is
ENG-10976/ENG-11067; the fix is ix#9073 and it is on ix main but not in the
revision these hosts are running. Nothing in this directory affects it.

Worth knowing while reading a `systemctl` output during this: the proxy unit
reads `active` throughout. hyperion-proxy binds its listener at startup and
dials the game server per connection, so a completely unreachable backend is
invisible to systemd. The unit crash-looped when the *name* would not resolve
and is quiet now that only the *path* is broken, which inverts the usual
relationship between how loud a failure is and how bad it is.

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

- **Cross-host east-west has no data path.** ENG-10976, ENG-11067. Measured
  above; this is now the only thing between this fleet and a player joining.
  The fix is on ix main and not deployed.
- **A group's DNS gateway is not consistently present.** ENG-11226. On the
  `hyperion` group's bridge, `fd00:1:1f:856f::1` is on hil-compute-1 and
  hil-compute-3 and absent on hil-compute-2, which carries two members of the
  same group. Latent while cross-host traffic is broken anyway, and a hard
  failure for a member placed on hil-compute-2 once it is not.
- **A public IPv4 currently disconnects the VM, so it is off.** ENG-10881. The
  region's ingress block is not delivered to the vRack: `15.204.22.254`, the
  block's gateway, is still `FAILED` in all three hil hosts' neighbour tables
  as of 2026-07-29, and `ip route show table 200` still points at it. Because
  the host source-routes VIP traffic out of that gateway, a VM that takes an
  address from the block can send nothing at all:

  ```
  hyperion-proxy-0  (VIP 15.204.22.195/32)  ping 1.1.1.1 -> 0 received, +2 errors
  hyperion-game     (no VIP, 10.0.0.59)     ping 1.1.1.1 -> 2 received, 0.700ms
  ```

  `ix.networking.ipv4` is therefore off in `proxy.nix` with the reasoning in
  place. Turn it back on and recreate the proxies once OVH delivers the block;
  the address is allocated at create and there is no `ix vm set --ipv4`.

  Public IPv6 is not a substitute: a guest's `/128` answers only from the host
  it lives on (ENG-11144). The interim that does reach the internet is
  `services.ix.vmPublicIngress` (ENG-11132), a DNAT from a host's own routed
  `bond0` address, declared in that host's inventory rather than here because it
  names a vmId and pins the VM to that host.
- **The proxies may all be on one host and nothing can ask otherwise.**
  ENG-11225, above.
- **No DNS name in front of the endpoint.** ENG-11218, gated on ENG-11222,
  shape above.
- **A fleet cannot boot its own published image.** ENG-10839. `mkFleet` renders
  one per node at `packages.<node>`, and nothing outside the private ix repo can
  build it, which is why the export cost above is per-VM rather than a single
  push into the region's CAS.
- **Nothing in CI evaluates this example.** ENG-10986: `exampleFleetsFor` skips
  it because it depends on an external flake, so the eval below is a command
  someone has to remember to run rather than a gate.

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
