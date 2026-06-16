# remote-desktop

`modules/services/remote-desktop/default.nix` exposes a browser-accessible
remote desktop backed by Xpra's built-in HTML5 client. The default session is an
icewm window manager with an xterm, built as a Nushell launcher.

Option namespace: `services.remote-desktop` (`default.nix:68`).

## Public surface (options)

- `enable` (`default.nix:69`).
- `package` (default `pkgs.xpra`) (`default.nix:71`).
- `port` (port, default 6080) - HTML5 client port (`default.nix:73`).
- `bindAddress` (str, default `0.0.0.0`) (`default.nix:79`).
- `openFirewall` (bool, default false) (`default.nix:85`).
- `display` (str, default `:100`) - X display Xpra manages (`default.nix:91`).
- `resolution` (str, default `1920x1080`) (`default.nix:97`).
- `desktopCommand` (str, default the bundled icewm+xterm session)
  (`default.nix:103`).
- `auth` (str, default `none`) - Xpra auth module (`default.nix:110`).
- `allowUnauthenticated` (bool, default false) - explicit opt-in to open the
  firewall while `auth = "none"` (`default.nix:116`).
- `settings` (attrs of bool/int/str/list) - raw `xpra start-desktop` flags
  rendered via `lib.cli.toCommandLineGNU` (`default.nix:126`). Convenience
  options seed this set via `mkDefault` (`default.nix:165-182`): `start`,
  `bind-tcp`, `auth`, `resize-display`, `socket-dirs`, `html=on`, `ssl=off`,
  `daemon=no`, and various feature toggles (clipboard on; pulseaudio,
  notifications, webcam, printing, file-transfer off).

## Key internals

- The launcher (`ix-remote-desktop`, a `writeNushellApplication`) execs
  `xpra start-desktop <display> <flags...>` (`default.nix:46-65`).
- Two assertions keep the endpoint coherent (`default.nix:143-156`): opening the
  firewall with the rendered auth `none` requires `allowUnauthenticated`; and the
  rendered `settings.bind-tcp` must equal `<bindAddress>:<port>` so Xpra, the
  port claim, and the firewall all use one endpoint.

## What it produces

- `ix.networking.portClaims.remote-desktop` (tcp, address `bindAddress`)
  (`default.nix:158`). Firewall on `port` when `openFirewall`
  (`default.nix:190`). No health check.
- A dedicated `remote-desktop` system user/group with home
  `/var/lib/remote-desktop` (`default.nix:192-198`).
- `systemd.services.remote-desktop` (`default.nix:200`): runs as that user,
  `HOME=/var/lib/remote-desktop`, `StateDirectory`/`RuntimeDirectory =
  remote-desktop`, `ExecStart` = the launcher.
- `environment.systemPackages` adds `xpra`, `icewm`, `xterm`
  (`default.nix:184`).

## How it is wired

Auto-discovered as `services/remote-desktop`. Runs `pkgs.xpra`; no flake output.
