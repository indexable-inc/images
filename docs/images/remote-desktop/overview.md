# remote-desktop

`images/desktop/remote-desktop` builds a browser-accessible graphical desktop:
an Xpra server rendering its built-in HTML5 client, so an operator opens the VM's
desktop in a web browser with no native client. Flake output `.#remote-desktop`.

## What it builds

`images/desktop/remote-desktop/default.nix` is the entire image (16 lines). It:

- names the image `ix-remote-desktop` (`default.nix:4`).
- ships `xterm` and `firefox` so first boot has a terminal and a browser
  (`default.nix:6-9`). `firefox` is the GUI app you exercise the desktop with.
- turns on the desktop service with the firewall open and authentication off:

```nix
services.remote-desktop = {
  enable = true;
  openFirewall = true;
  allowUnauthenticated = true;   # default.nix:11-15
};
```

`allowUnauthenticated = true` is required here because `openFirewall = true` with
`auth = "none"` is otherwise an eval error: the module refuses to expose an
unauthenticated desktop to the network without the explicit opt-in
(`modules/services/remote-desktop/default.nix:116-124,143-147`). Treat this image
as trusted-network/per-tenant-VM only.

## Composed module: `services.remote-desktop`

Defined in `modules/services/remote-desktop/default.nix`. Key surface:

- `enable` (`:69`), `package` (default `pkgs.xpra`, `:71`).
- `port` (default 6080, `:73-77`), `bindAddress` (default `0.0.0.0`, `:79-83`),
  `openFirewall` (default false, `:85-89`).
- `display` (default `:100`, `:91-95`), `resolution` (default `1920x1080`,
  `:97-101`), `desktopCommand` (default an icewm session that spawns xterm,
  `:20-32,103-108`).
- `auth` (default `none`, `:110-114`) and `allowUnauthenticated` (`:116-124`).
- `settings` (freeform `xpra start-desktop` flags rendered by
  `lib.cli.toCommandLineGNU`, `:126-139`); the module seeds `bind-tcp`,
  `html = on`, `ssl = off`, `clipboard = on`, audio/printing/webcam off
  (`:165-182`).

Runtime wiring: registers an `ix.networking.portClaims.remote-desktop` TCP claim
on `port` (`:158-163`), opens the firewall when `openFirewall`
(`:190`), runs as a dedicated `remote-desktop` system user
(`:192-198`), and starts `systemd.services.remote-desktop` after
`network-online.target` from a Nushell launcher that execs
`xpra start-desktop <display> <flags>` (`:46-65,200-216`). `icewm` + `xterm` are
added to `environment.systemPackages` (`:184-188`).

## Build and access

```
nix build .#remote-desktop          # realize the OCI tar
```

After boot, Xpra listens on TCP 6080 (the module default `port`); the HTML5
client is served at that port. The base platform always also opens TCP 5001
(ix-console) and UDP 8443 (ix-agent); see [common](../common.md). For port-level
policy beyond on/off, set `networking.firewall.*` inside the image.

## Notes

- No eval test group is attached to this image name; the `services.remote-desktop`
  module assertions (firewall-vs-auth, `bind-tcp` alignment) are the eval gate
  (`modules/services/remote-desktop/default.nix:143-156`).
- To require auth, set `services.remote-desktop.auth` to a real Xpra auth module
  and drop `allowUnauthenticated`.
