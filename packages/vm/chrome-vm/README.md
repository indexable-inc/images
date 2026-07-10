<p align="center"><img src="assets/hero.svg" width="720" alt="one command boots a Linux guest that screenshots a page with headless Chromium and ships the PNG back over the serial console"></p>

# chrome-vm

Can a real browser really run inside a VM on your Mac, provably, with one
command? `chrome-vm` boots headless Chromium inside a real Linux VM on a macOS
host and gets the screenshot back:

```sh
nix run github:indexable-inc/index#chrome-vm
# or, from a clone (git clone https://github.com/indexable-inc/index):
nix run .#chrome-vm [out.png]
```

It boots an aarch64 Linux guest under [`vmkit`](../vmkit)/libkrun
(Hypervisor.framework), runs `chromium --headless` against a baked proof page,
and opens the PNG the guest captured. The page prints the live Chromium
user-agent, a fresh timestamp, and a canvas gradient drawn by JS, so the
screenshot is proof that a real browser ran real JS in the guest, not a
placeholder.

Self-contained: the guest needs no network, no GPU, and no host directory
sharing. The screenshot travels back as base64 over the guest's serial console,
which `vmkit --console-file` captures and the runner decodes.

## How it works

- **Guest** ([`../chrome-vm-image`](../chrome-vm-image)): an aarch64 NixOS image
  whose only job is a boot-time oneshot that screenshots the page with headless
  Chromium (software raster, no GPU), writes the PNG as `base64` straight to
  `/dev/console` between `===VMKIT-SHOT-BEGIN===`/`===VMKIT-SHOT-END===` markers,
  then powers off. Writing to `/dev/console` bypasses systemd's per-line console
  prefixing, which would otherwise corrupt the base64.
- **Disk**: assembled with `systemd-repart` (not `make-disk-image`), so it builds
  in the Nix sandbox with no qemu/kvm VM. libkrun-efi boots OVMF -> systemd-boot
  (at the EFI removable path) -> a UKI. `sectorSize = 512` is required (OVMF does
  not handle repart's 4096 default).
- **Runner** (this package): a macOS nushell app that builds the guest image,
  boots it with `vmkit boot-linux`, decodes the screenshot, and `open`s it.

## Requirements and limits

- Host is `aarch64-darwin` (Apple Silicon); `vmkit`'s Linux-guest path is
  libkrun-efi there.
- Building the guest image needs an **aarch64-linux builder**. hydra has none
  natively, so it offloads to the local OrbStack remote builder (see
  `~/.config/nix` `hosts/hydra`). The first run also fetches Chromium's closure
  (~1.5 GiB), so it is slow once, then cached.
- Override the image source flake for local testing:
  `IX_CHROME_VM_FLAKE=/path/to/checkout nix run .#chrome-vm`.
