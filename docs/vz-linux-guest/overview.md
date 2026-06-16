# vz-linux-guest

`packages/vz-linux-guest` is the raw EFI-bootable aarch64 NixOS disk image that
[vmkit](../vmkit/overview.md)'s `boot-linux-gui` / `drive-linux` path boots under
Apple's Virtualization.framework (VZ) on Apple Silicon. It boots straight into a
Wayland compositor running [bossbar-overlay](../bossbar-overlay/overview.md)
on software graphics, so the host can screenshot a real Linux GUI render off the
VZ framebuffer (`nixos.nix:1-7`). It is the Linux-GUI counterpart to
[chrome-vm-image](../chrome-vm-image/overview.md) (which is headless under
libkrun): this one targets VZ specifically because VZ is the only working
Linux-GUI framebuffer-capture path (see
[vmkit linux-guests](../vmkit/linux-guests.md)).

## What it is

- Nix-only package, `aarch64-linux` only (`package.nix:7-8`). Flake output
  `vz-linux-guest`: `nix build .#packages.aarch64-linux.vz-linux-guest`. The
  output is the raw disk image.
- Built by evaluating a standalone NixOS system from nixpkgs' `eval-config.nix`
  (one module `./nixos.nix`, the overlaid aarch64-linux `bossbar-overlay` passed
  as a `specialArg`) and rendering it with `make-disk-image`
  (`default.nix:14-28`): `format = "raw"`, `partitionTableType = "efi"`,
  `additionalSpace = "512M"` headroom. No nixos-generators/disko input needed.

`make-disk-image` (unlike `chrome-vm-image`'s `systemd-repart`) is used here; it
labels the partitions `ESP` and `nixos`, which the system mounts by label
(`nixos.nix:64-74`).

## Boot and display (`nixos.nix:38-62`)

- Boots under VZ's EFI firmware (`VZEFIBootLoader`) off the raw disk via
  systemd-boot (`boot.loader.systemd-boot.enable = true`, `timeout = 0`,
  `efi.canTouchEfiVariables = false`).
- `kernelParams = [ "console=tty0" "console=hvc0" ]`: tty0 writes to the
  virtio-gpu framebuffer; hvc0 (the serial console) is streamed to the host by
  `boot-linux-gui` for boot debugging.
- initrd has `virtio_pci`, `virtio_blk`, `virtio_scsi`, `usbhid`, `sd_mod`; the
  `virtio_gpu` DRM module is loaded so the compositor runs on the paravirtual
  display whose scanout VZ exposes to the host framebuffer.

## Software graphics

Apple's virtio-gpu has no 3D acceleration, so the whole GUI stack runs on
software rendering (`nixos.nix:5-7,76-81`):

- the compositor (sway/wlroots) uses the pixman software renderer
  (`WLR_RENDERER = "pixman"`, `WLR_RENDERER_ALLOW_SOFTWARE = "1"`).
- the wgpu overlay uses Mesa lavapipe (software Vulkan). The launch wrapper
  `bossbarLaunch` (`nixos.nix:24-29`) sets `VK_DRIVER_FILES` to the lavapipe ICD
  (`lvp_icd.aarch64.json`) and `LD_LIBRARY_PATH` to Mesa, which is mandatory:
  with no ICD wgpu hard-panics and there is no GL fallback. It also seeds a fresh
  `BOSSBAR_DB=/tmp/bossbars.db` so the overlay auto-populates demo bars.

## Autologin to the overlay (`nixos.nix:31-98`)

The image logs in automatically and lands in the overlay with no interaction:

- `services.getty.autologinUser = "ix"` (a passwordless normal user in the
  `wheel`/`video`/`input` groups).
- `programs.bash.loginShellInit` execs `sway -c <swayConfig>` when the tty is
  `/dev/tty1`.
- the sway config (`nixos.nix:32-36`) sets a fixed `1920x1080` output, no border,
  and `exec`s `bossbarLaunch` as the only client.

`environment.systemPackages` includes `sway`, `grim` (screenshots), `mesa`,
`vulkan-loader`, `vulkan-tools`, and `bossbar-overlay` (`nixos.nix:100-107`).
The image is trimmed: `documentation.enable` and `networking.useDHCP` are off
(`nixos.nix:109-112`).

## Related

- [vmkit linux-guests](../vmkit/linux-guests.md): the VZ GUI-capture path
  (`boot-linux-gui`/`drive-linux`) that boots this disk, and why it stays on VZ
  rather than libkrun.
- [bossbar-overlay](../bossbar-overlay/overview.md): the wgpu GUI app
  this guest runs (verifying it renders on Linux without a real GPU).
