# panes: seamless guest-Linux windows on macOS

Run Linux GUI apps in a lightweight VM on Apple Silicon and give each app its
own **native macOS window**, WSLg-style, instead of one big "VM screen".
Tracking issue: [index#1686](https://github.com/indexable-inc/index/issues/1686).

## How it fits together

```
macOS host                          aarch64-linux guest (libkrun-efi, vmkit)
+------------------+   vsock 7100   +--------------------------------------+
| panes-host       |<-------------->| panes-compositor (headless Wayland)  |
|  one NSWindow    |  panes-protocol|   socket: /run/panes/wayland-1       |
|  per toplevel,   |  postcard      +--------------------------------------+
|  input forwarded |  frames        | nspawn containers, one per app       |
+------------------+                |   demo | term | minecraft | ...      |
                                    |   each binds /run/panes + /dev/dri   |
                                    +--------------------------------------+
```

- [`protocol/`](protocol/) (`panes-protocol`): the wire format. One duplex
  byte stream (guest vsock port 7100 to a host unix socket) carrying
  length-prefixed postcard frames: damage-tiled window contents up, input and
  configure events down, ack-driven pacing genlocked to the host display. The
  crate doc comments are the protocol spec. Minor 1 (v1.1) adds pointer lock
  for mouse-look apps (index#1724): `ToHost::PointerLock` when a surface's
  `zwp_locked_pointer_v1` (de)activates, `ToGuest::PointerRelative` for the
  raw deltas the host forwards while its cursor is captured; both are gated
  on the peer's Hello minor (postcard tolerates no unknown variants).
- [`compositor/`](compositor/) (`panes-compositor`): guest-side headless
  Wayland compositor. Each `xdg_toplevel` becomes a window on the host; no
  DRM output, no seat, no logind, it composites nothing and only exports.
- [`host/`](host/) (`panes-host`): macOS agent that owns the NSWindows and
  forwards input back.
- [`guest-image/`](guest-image/) (`panes-guest-image`): the bootable
  aarch64-linux NixOS raw-efi disk wiring it all up: the compositor as a
  systemd service plus one autostarted systemd-nspawn container per app.

## The guest image

Built like `packages/vm/chrome-vm-image`: systemd-repart assembles the disk in
the nix sandbox (no qemu/kvm needed on the builder), systemd-boot at the EFI
removable path boots a UKI, and libkrun-efi's OVMF firmware boots the disk.

```sh
nix build .#packages.aarch64-linux.panes-guest-image
# The store image is minimized (no free space); copy it out and enlarge it,
# the guest grows the root partition + filesystem into the new space at boot
# (boot.growPartition + autoResize). Minecraft downloads land in that space.
cp --sparse=always ./result /tmp/panes-guest.raw && chmod +w /tmp/panes-guest.raw
truncate -s 8G /tmp/panes-guest.raw
nix run .#vmkit -- boot-linux --disk /tmp/panes-guest.raw --gpu --net --memory-mib 6144 --cpus 6
```

Size the guest for Minecraft: it OOMs under ~4 GiB of guest RAM and crawls on
2 vCPUs; `--memory-mib 6144 --cpus 6` is the validated boot line.

GPU: `--gpu` attaches libkrun's virtio-gpu **venus** device (Vulkan on the
Mac's GPU via MoltenVK). The image loads `virtio_gpu`, ships mesa's venus ICD
through `/run/opengl-driver`, and logs `vulkaninfo --summary` to the serial
console at boot (the `panes-venus-smoke` oneshot): with `--gpu` it must show a
`Virtio-GPU Venus` device, lavapipe-only output means the venus path is
broken. Minecraft (Java 26.2+) uses its first-party Vulkan renderer directly
on venus, no zink and no mods; the image pre-seeds its `options.txt` with
`preferredGraphicsBackend:"vulkan"`.
Details: [`vmkit/docs/linux-libkrun.md`](../vmkit/docs/linux-libkrun.md).

> **Gap**: the shipped `pkgs.panes-compositor` builds without the `gpu` cargo
> feature (the unit graph has no feature knob yet), so no linux-dmabuf global
> is advertised and Vulkan/GL clients fall back to shm or software. The venus
> plumbing above (16K kernel, ICD, `/dev/dri` binds) is ready; enabling the
> feature in the guest-image build is the remaining step for accelerated
> window content (index#1686).

Networking: `--net` runs gvproxy (`192.168.127.0/24`); the image DHCPs its
NIC. App containers share the host network namespace, so e.g. Minecraft can
download its assets on first launch.

## Adding an app

Apps are data, not modules: add an entry to
[`guest-image/apps.nix`](guest-image/apps.nix) with a `command`, optional
`env`, and optional persistent `binds`. The machinery in
[`guest-image/nixos.nix`](guest-image/nixos.nix) renders each entry into an
autostarted nspawn container that bind-mounts the compositor socket
(`/run/panes/wayland-1`) and `/dev/dri`, and exports
`WAYLAND_DISPLAY`/`XDG_RUNTIME_DIR`. One entry, one container, one window.

## Status

The protocol crate is real; `panes-compositor` and `panes-host` are stubs
being filled in (M2/M3 in index#1686). The guest image already wires the
compositor service and the app containers, so it boots today with a legible
"not yet implemented" from the compositor unit on the serial console, and the
containers retry until the socket appears.
