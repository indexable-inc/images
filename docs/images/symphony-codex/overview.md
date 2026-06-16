# symphony-codex

`images/dev/symphony-codex` is a disposable VM that runs Symphony agent jobs: a
Codex/Claude agent workspace with a RAM-backed `/workspace`, the room-server
network ports declared, and the agent + repo tooling needed to drive a run.
Flake output `.#symphony-codex`.

## What it builds

`images/dev/symphony-codex/default.nix` (111 lines):

- `ix.image = { name = "ix/symphony-codex"; tag = "2026-05-31"; }` (`:20-23`)
  and `networking.hostName = "symphony-codex"` (`:25`).
- backs `/workspace` with a sized tmpfs (`:40-47`):

```nix
fileSystems."/workspace" = {
  device = "tmpfs"; fsType = "tmpfs";
  options = [ "size=32g" "mode=0755" ];
};
```

  Symphony clones each run's repo checkouts under `/workspace`. The VM's writable
  root is virtiofs-from-CAS, whose small-file write path is slow (~5-6k files/s,
  collapsing under host load), so a full `git checkout` of the ~9k-file repo onto
  the root never finishes inside the provision timeout (ENG-2007/ENG-2004). tmpfs
  writes into guest RAM (~36k files/s) and the node-agent virtio-mem autoscaler
  grows guest RAM on demand; `size=32g` is a lazily-allocated ceiling, not a
  reservation (`:27-39`).

- ships its own wrapped Claude binary (`makeWrapper ... --set IS_SANDBOX 1`,
  `:8-17,59`), the same narrow sandbox-signal wrapper [development-base](../development-base/overview.md)
  uses, because Symphony agents run as root in this disposable VM.
- `environment.systemPackages` (`:55-89`): `claude-code` (wrapped), `pkgs.codex`,
  the repo `mcp` server (`ix.packages.mcp`, `:76`), `ix.packages.pi-harness`
  (`:79`), `nodejs_24`, `python3`, plus the agent workspace toolchain
  (`ast-grep`, `bashInteractive`, `cacert`, `cmake`, `curl`, `direnv`, `fd`,
  `findutils`, `gcc`, `gh`, `git`, `gnugrep`, `gnumake`, `gnused`, `gnutar`,
  `gzip`, `jq`, `openssh`, `pkg-config`, `ripgrep`, `unzip`, `which`, `zstd`).

## Room-server networking

The image opens and claims the Symphony room-server ports even though the
room-server package itself is not currently shipped (`:91-110`):

- TCP 8080: `symphony-room-http` (`networking.firewall.allowedTCPPorts = [ 8080 ]`,
  `ix.networking.portClaims.symphony-room-http`, `:92,96-102`).
- UDP 4433: `symphony-room-webtransport`
  (`networking.firewall.allowedUDPPorts = [ 4433 ]`,
  `ix.networking.portClaims.symphony-room-webtransport`, `:93,104-109`).

The room-server (`pkgs.symphony-room-server`) is a `TODO` pending re-add: its
real home is the ix monorepo (`crates/room`), and ix already inputs index, so
index cannot source it without a circular flake dependency (`flake.nix:131-137`,
`default.nix:83-85`). The ports/claims stay so re-adding the package is a one-line
change.

## Build

```
nix build .#symphony-codex
```

## Eval test (`tests/default.nix:3376-3426`)

Asserts the image includes `codex`, `gh`+`git`, and common agent workspace tools
(`direnv`, `ripgrep`); opens room-server HTTP (8080) and WebTransport (4433) on
top of the base firewall ports; registers the two room listener port claims with
the right protocol/port; and backs `/workspace` with a `tmpfs` carrying
`size=32g`. The room-server-presence assertion is a `TODO` parked until the
package is restored (`:377-379`).
