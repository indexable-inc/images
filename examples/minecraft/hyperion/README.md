# hyperion: one game server, two proxies

```
players -> hyperion-proxy-0 \
                             >-- hyperion-game (private, no public address)
players -> hyperion-proxy-1 /
```

```sh
nix build .#hyperion-game-system .#hyperion-proxy-0-system .#hyperion-proxy-1-system
ix apply .#hyperion-game .#hyperion-proxy-0 .#hyperion-proxy-1
```

Change anything and run the same command again. Each VM is reused by name and
switched in place, so only the units whose definition changed restart.

## Build the fleet once, not once per VM

The `nix build` line is what makes the apply finish in minutes instead of
hours, and it is the only reason for the `packages` output in `flake.nix`.

Without it, `ix apply` hands each VM the derivation and each VM realises it in
its own store. The three nodes here are one closure with three hostnames:

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

What this does not fix: the closure crosses your uplink once per VM. Three
nodes sharing one closure still pay three exports. The shape that avoids that
is one push into the region's CAS and N VMs materializing from it, which is
what ENG-10839 is about.

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

## State of the deployment, 2026-07-28

The three VMs exist in `us-west-1`. `hyperion-game` reaches ready. The proxies
switch to their new system and then crash-loop on one thing, and it is not
anything this directory can spell differently.

Measured on a full apply from no VMs at all, with the two-line flow at the top:

    nix build, all three systems     195s, 78 derivations, one remote builder
    ix apply, all three nodes        1842s (30m42s)
      VM create                        1.6s, 1.0s, 1.7s, "reusing golden snapshot"
      closure export                   ~27m, about 2.9 GiB per VM, uplink bound
      activation                       ~2m
    ix apply again, unchanged        156s, no bytes crossing

Compare with the apply that motivated all of this: 83 minutes, three guests
compiling the same closure independently, and it never finished.

The 27 minutes is the honest remaining cost and it is not a build: it is the
client exporting each guest's missing store paths over one home uplink, three
times, for one closure. See the section above for why an image push is the shape
that removes it, and ENG-10839 for why you cannot do that yet.

Fixed since this example landed:

1. **Groups can be created, and a first apply joins them.** `ix group create`
   failed because `vm_groups` is a regional table with a foreign key into
   globally owned identity (ix#8841). `ix apply` also reads
   `ix.networking.groups` off the evaluated system now, so the group and its
   membership come up on a first apply from nothing: `ix group ls` shows
   `hyperion` with all three nodes in it. `hyperion.gameAddress` was the
   workaround and is gone, and the old advice to create each VM by hand with
   `--group` is gone with it.
2. **The game server reaches `active`.** Four faults, all fixed in #4246: the
   PKI material was unreadable by the two `DynamicUser` services that need it,
   the world database opened relative to a read-only working directory, `$HOME`
   was unset so the world cache had nowhere to go, and the listen address was
   built by string concatenation.
3. **The listen address is a `SocketAddr`.** hyperion#990 gives both events one
   launcher, so `--ip ::` reaches the socket instead of panicking. The `[::]`
   workaround this file carried is deleted.
4. **A guest no longer has to build.** With the systems built first, each switch
   goes straight to `importing closure`; nothing compiles in a VM. The three
   faults this file used to list as "properties of one artifact" (ENG-10487
   `nix-daemon.socket`, ENG-10512 store paths owned by nobody, ENG-10522 the
   14 GB root) were all about a guest trying to build. None of them appeared in
   this run.

Open:

- **Group members cannot resolve each other.** ENG-10855, and it is the only
  reason the proxies are not serving. `hyperion-proxy` exits 1 on
  `failed to lookup address information`, because `hyperion-game.ix.internal`
  does not resolve inside the guest: `/etc/resolv.conf` is `1.1.1.1`, the group
  DNS view is not wired to the guest, and the boot path passes
  `use_internal_dns: false`. That single crash-looping unit is the whole of the
  `switch-to-configuration exited with status 4`; `/run/current-system` on both
  proxies is the new system and `systemctl --failed` lists nothing. The name is
  load bearing (see above), so an address here would fail the TLS handshake
  instead.
- **The proxies have no public address.** ENG-10846. `default.ix` asks for one
  with `deployment.ipv4`, which only the deprecated `ix-fleet` reads;
  `ix apply` reads `ix.networking.ipv4` off the evaluated system, and this
  directory's `flake.lock` pins an index that has no such option yet. The fix is
  a lock bump plus a one-line move into `proxy.nix`, and it wants its own change
  because it moves every node's closure.
- **A fleet cannot boot its own published image.** ENG-10839. `mkFleet` renders
  one per node at `packages.<node>`, and nothing outside the private ix repo can
  build it, which is why the 27 minutes above is a per-VM export rather than a
  single push into the region's CAS.
