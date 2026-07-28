# darwin-deploy

Deploy nix-darwin configurations to remote macOS hosts, colmena-style.
`darwin-rebuild` has no `--target-host`, so remote Macs (guest VMs, CI
runners) otherwise drift to imperative scripts.

```sh
darwin-deploy --flake github:owner/repo mac1=admin@192.168.64.6 mac2=ci@mac2.local
```

For each `name=[user@]host` node, in parallel:

1. `nix build` `darwinConfigurations.<name>.system` locally
2. `nix copy --to ssh://<host>` the closure
3. remotely set `/nix/var/nix/profiles/system` and run the nix-darwin
   activation scripts (legacy `activate-user` as the ssh user when the
   generation still ships a live one, then `activate` as root)

Remote root steps run under `sudo --set-home`, so the ssh user needs
passwordless sudo (or connect as `root`). `nix copy` pushes unsigned local
builds, so the ssh user should be a nix `trusted-user` on the target.
All ssh runs with `BatchMode=yes`: missing keys fail loudly instead of
prompting.

`--dry-run` builds and reports each host's current vs built system without
copying or activating. `--json` emits one machine-readable report document
on stdout; human logs stream to stderr either way. The exit code is non-zero
if any node failed, and one node's failure never blocks the others.

One-time bootstrap (installing nix on a fresh host, TCC grants) is out of
scope; see the nix-darwin manual.
