# chrome-vm

`packages/chrome-vm` runs headless Chromium inside a real Linux VM on a macOS
host and gives the screenshot back, in one command. It is the host-side runner
that wires the [chrome-vm-image](../chrome-vm-image/overview.md) guest disk to
[vmkit](../vmkit/overview.md)'s libkrun Linux-guest path, then decodes and opens
the PNG the guest captured. It exists as an end-to-end proof that a real browser
runs real JS inside a libkrun guest booted by vmkit, not a placeholder.

```sh
nix run .#chrome-vm [out.png]        # default out: ./chrome-vm-shot.png
```

## What it is

- Nix-only package: a macOS nushell app built by
  `writeNushellApplication` (`packages/vm/chrome-vm/default.nix:25-90`). Not a Rust
  workspace member.
- Flake output `chrome-vm`, `aarch64-darwin` only (`package.nix:5-6`): the host
  must be Apple Silicon because vmkit's Linux-guest path there is libkrun-efi.
- `runtimeInputs`: the `vmkit` binary (reached through
  `ix.rustWorkspace.units.binaries."vmkit"`, since repo crates are not overlaid
  into `pkgs`, `default.nix:19-23`), plus `bash`, `gawk`, `coreutils`,
  `gnugrep`.

## Flow (`default.nix:34-89`)

1. **Build the guest image.** `nix build <flake>#packages.aarch64-linux.chrome-vm-image`
   produces the raw EFI disk (`default.nix:42-46`). Building it needs an
   aarch64-linux builder; on hydra that offloads to the local OrbStack remote
   builder. The source flake defaults to `github:indexable-inc/index` and is
   overridable with `IX_CHROME_VM_FLAKE=/path/to/checkout` for local testing
   (`default.nix:36`).
2. **Copy the disk writable.** libkrun needs a writable disk; the Nix store
   image is read-only, so it is copied to a temp dir and `chmod u+w`
   (`default.nix:48-51`).
3. **Boot under vmkit.** `vmkit boot-linux --disk <disk> --console-file <log>
   --memory-mib 2048 --cpus 4 --timeout-secs 150` (`default.nix:54-61`). The
   guest screenshots on boot, prints the PNG as base64 over the serial console,
   and powers off; vmkit captures the console to `<log>`. A nonzero vmkit exit is
   swallowed because vmkit `exit()`s 0 on both clean poweroff and timeout, so the
   PNG check below is the real success gate.
4. **Decode the screenshot.** `awk` extracts the base64 between the
   `===VMKIT-SHOT-BEGIN===`/`===VMKIT-SHOT-END===` markers, strips whitespace,
   and `base64 -d`s it to the output path (`default.nix:66-68`).
5. **Verify and open.** Success requires the output to start with the PNG magic
   `89 50 4E 47 0D 0A 1A 0A` (guards a truncated decode); on success it prints
   the guest's demo banner lines and `open`s the PNG, on failure it dumps the
   last 40 console lines and exits 1 (`default.nix:70-88`).

## Why it is self-contained

The guest needs no network, no GPU, and no host directory sharing: the
screenshot travels back as base64 over the guest's serial console, which
`vmkit --console-file` captures and this runner decodes. That is the whole
transport. See [chrome-vm-image](../chrome-vm-image/overview.md) for the guest
side (the boot-time Chromium oneshot and why it writes to `/dev/console`).

## Requirements and limits (`README.md:36-45`)

- Host is `aarch64-darwin` (Apple Silicon); vmkit's Linux-guest path is
  libkrun-efi there.
- Building the guest image needs an aarch64-linux builder; the first run also
  fetches Chromium's closure (~1.5 GiB upstream estimate), so it is slow once,
  then cached.
- `IX_CHROME_VM_FLAKE=/path/to/checkout` overrides the image source flake for
  local testing.

## Related

- [vmkit](../vmkit/overview.md) / [vmkit linux-guests](../vmkit/linux-guests.md):
  the `boot-linux` backend and `--console-file` capture this depends on.
- [chrome-vm-image](../chrome-vm-image/overview.md): the guest disk booted here.
