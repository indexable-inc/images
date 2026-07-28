# chrome-vm-image

`packages/chrome-vm-image` is the raw EFI-bootable aarch64 NixOS disk image that
the [chrome-vm](../chrome-vm/overview.md) demo boots under
[vmkit](../vmkit/overview.md)/libkrun. The guest's only job is a boot-time
oneshot that screenshots a baked proof page with headless Chromium, base64-encodes
the PNG onto the serial console, and powers off; the host runner decodes the
base64 between markers into a PNG.

## What it is

- Nix-only package, `aarch64-linux` only (`package.nix:7-8`). Flake output
  `chrome-vm-image`: `nix build .#packages.aarch64-linux.chrome-vm-image`. The
  package output is the raw disk file itself (`chrome-vm.raw`), copied out of the
  evaluated system's repart image (`default.nix:20-24`).
- Built by evaluating a standalone NixOS system from nixpkgs' `eval-config.nix`
  with the one module `./nixos.nix` (`default.nix:14-19`); `path` is `pkgs.path`
  supplied by the package autoArgs.

## How the disk is built: systemd-repart, not a VM

The disk is assembled with **systemd-repart** (the `image/repart.nix` module
imported at `nixos.nix:56`), not `make-disk-image`. repart runs in the Nix build
sandbox with no nested qemu/kvm VM, so the image builds on a plain aarch64-linux
builder with no `/dev/kvm` (e.g. hydra's OrbStack remote builder)
(`default.nix:6-9`, `nixos.nix:6-11`).

Boot path: libkrun-efi's embedded OVMF -> systemd-boot (placed at the EFI
removable path) -> a UKI in `/EFI/Linux` (`nixos.nix:58-59`). The repart layout
(`nixos.nix:89-127`):

- `sectorSize = 512` is **required**: OVMF does not handle repart's 4096-byte
  default (`nixos.nix:91-92`).
- an `esp` (`Type = esp`, vfat, 256 MiB) holds `BOOTAA64.EFI` (systemd-boot
  aa64), the UKI, and a `loader.conf` with `timeout 0` to auto-boot the single
  UKI with no menu (`nixos.nix:94-116`).
- a `root` partition (`Type = root`, ext4, label `root`, `Minimize = "guess"`)
  holds the system closure; the rootfs is found by GPT partition label
  `/dev/disk/by-partlabel/root` (`nixos.nix:78-82,117-125`).

grub is disabled since the bootloader + UKI are placed manually by repart
(`nixos.nix:60-61`).

## The screenshot oneshot (`nixos.nix:129-182`)

`systemd.services.chrome-shot` is a `oneshot` wanted by and after
`multi-user.target`. It runs Chromium against a baked HTML proof page and writes
the result to the console:

- The page (`demoPage`, `nixos.nix:23-53`) prints the live Chromium user-agent,
  a fresh timestamp, and a JS-drawn canvas gradient, so a blank/placeholder
  capture is obvious.
- Chromium runs `--headless=new --no-sandbox --disable-gpu` with software raster
  (no GPU), `--virtual-time-budget=2500` so the page JS runs before the shot, and
  `--screenshot=/tmp/shot.png` at `1280x800` (`nixos.nix:166-170`).
- The script `exec`s its stdout/stderr straight to `/dev/console`
  (`nixos.nix:155`), bypassing systemd's `journal+console` connector. That
  connector prefixes every line with `[ts] chrome-shot[pid]: `, whose
  alphanumerics would survive a non-base64 strip and corrupt the decode.
- It emits `===VMKIT-CHROME-DEMO===` plus `uname`/`chromium --version`, then the
  PNG between `===VMKIT-SHOT-BEGIN===` / `===VMKIT-SHOT-END===` as one
  `base64 -w0` line (or `===VMKIT-NO-SHOT===` on failure), then
  `systemctl poweroff` so vmkit returns (`nixos.nix:161-180`).

## Keeping the base64 line clean

The serial console (`hvc0`) carries both kernel output and the single long
base64 line, so a stray kernel `printk` mid-line would corrupt the decode. The
image defends against this two ways: `console=hvc0 loglevel=0` keeps kernel
printk off the console at boot (`nixos.nix:66-69`), and the oneshot writes
`1` to `/proc/sys/kernel/printk` to drop the console level to emergency-only
before emitting the base64 (`nixos.nix:158`). The needed virtio modules
(`virtio_pci`, `virtio_blk`, `virtio_console`, `sd_mod`) are in the initrd
(`nixos.nix:70-75`).

The image is trimmed for an offline, no-GUI demo: `documentation.enable` and
`networking.useDHCP` are off, and `NetworkManager-wait-online` is disabled so
boot does not wait on a network the demo never uses (`nixos.nix:184-191`).

## Related

- [chrome-vm](../chrome-vm/overview.md): the macOS host runner that builds this
  image, boots it with `vmkit boot-linux --console-file`, and decodes the shot.
- [vmkit linux-guests](../vmkit/linux-guests.md): the libkrun-efi backend and
  the embedded OVMF that boots this disk.
