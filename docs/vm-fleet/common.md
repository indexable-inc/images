# vm-fleet

VM lifecycle, fleet tooling, guest images, and process/kernel debugging for the
index repo. This domain owns the code that boots and drives a guest virtual
machine (`vmkit`), the disk images those VMs run (`chrome-vm-image`,
`vz-linux-guest`), an end-to-end demo runner that ties a host to a guest
(`chrome-vm`), the CLI that converges a fleet of remote ix VMs from a declared
plan (`ix-fleet`), and the live-memory debugger packaged for the base system
image (`drgn`). The unifying concern is a host running guest VMs and the tooling
to start, drive, image, fleet-manage, and debug them.

Read this page first, then the component page for the unit you are touching.
[vmkit](vmkit/overview.md) is the load-bearing component; most others build on
it.

## Units

| unit | kind | role |
| --- | --- | --- |
| `packages/vmkit` | Rust workspace binary (`nix run .#vmkit`) | Own a guest VM lifecycle from Rust: macOS guests on Virtualization.framework, Linux guests on libkrun. See [vmkit](vmkit/overview.md). |
| `packages/ix-fleet` | Nix-only Python CLI (`nix run .#ix-fleet`) | Render and execute declarative fleet plans against the ix control plane (create/switch/replace/health-check remote VM nodes). See [ix-fleet](ix-fleet/overview.md). |
| `packages/chrome-vm` | Nix-only nushell app (aarch64-darwin) | One-command demo: boot a Linux guest under `vmkit`, run headless Chromium inside it, open the screenshot the guest captured. See [chrome-vm](chrome-vm/overview.md). |
| `packages/chrome-vm-image` | Nix-only NixOS disk image (aarch64-linux) | The raw EFI guest disk `chrome-vm` boots: a boot-time Chromium screenshot oneshot. See [chrome-vm-image](chrome-vm-image/overview.md). |
| `packages/vz-linux-guest` | Nix-only NixOS disk image (aarch64-linux) | The raw EFI Linux GUI guest for `vmkit boot-linux-gui` (a sway compositor running `bossbar-overlay` on software graphics). See [vz-linux-guest](vz-linux-guest/overview.md). |
| `packages/drgn` | Nix-only Python app wrapper (Linux) | Programmable debugger for live processes and kernels over `/proc/kcore`; shipped in the base system profile. See [drgn](drgn/overview.md). |

Only `vmkit` is a Rust workspace member (root `Cargo.toml`); the rest are
Nix-only packages. `ix-fleet` is a `uv`-built Python application; `chrome-vm` is
a nushell wrapper; `chrome-vm-image` and `vz-linux-guest` are NixOS systems
rendered to raw disks; `drgn` is a `buildPythonApplication` repackage of an
upstream release.

## Host, guest, and VM-backend relationships

Two host platforms and two guest OS families pair with three hypervisor
backends. The choice of backend is fixed by the host+guest pair, not a runtime
flag:

```
host = macOS (aarch64-darwin)                         host = Linux (aarch64/x86_64)
  guest = macOS        -> Virtualization.framework      (no macOS guest)
  guest = Linux (GUI)  -> Virtualization.framework      (no VZ GUI path)
                          (software GPU: lavapipe)
  guest = Linux (head) -> libkrun-efi                  guest = Linux -> libkrun (KVM)
                          (Hypervisor.framework;          (bundled libkrunfw kernel;
                           GPU via Venus/MoltenVK)         rootfs + exec; /dev/kvm)
```

- **Virtualization.framework (VZ)** is macOS-host only and the only backend for
  macOS guests. It also drives the off-screen Linux **GUI** capture path, where
  it gives the guest no 3D acceleration (the guest falls back to Mesa lavapipe
  software Vulkan). VZ is the only working framebuffer-capture path today.
- **libkrun** runs Linux guests headless on both hosts, with a different library
  per host: `libkrun-efi` (Hypervisor.framework, boots an EFI disk, GPU via a
  virtio-gpu Venus device backed by MoltenVK) on a macOS host, and classic KVM
  `libkrun` (boots a rootfs directory under a bundled `libkrunfw` kernel) on a
  Linux host. See [vmkit/linux-guests](vmkit/linux-guests.md).
- A guest gets a **GPU or Rosetta, never both**: the GPU is a libkrun feature,
  Rosetta is a VZ feature.

`vmkit` is the single binary that owns all of these. On macOS it carries the
virtualization/hypervisor entitlements by self-signing into a per-user cache, so
callers that cannot hold entitlements (notably the ix-mcp Python interpreter)
spawn it instead of driving a hypervisor in-process. `vmkit` is also exposed as
an ix-mcp tool provider; that Python module lives in the
[mcp](../mcp/tool-providers/overview.md) domain and is not documented here.

The two image packages produce guest disks for `vmkit`'s Linux paths:
`chrome-vm-image` is booted headless by libkrun (`boot-linux`), `vz-linux-guest`
is booted off-screen by VZ for GUI capture (`boot-linux-gui`). `chrome-vm` is
the host-side runner that wires `chrome-vm-image` to `vmkit`.

`ix-fleet` and `drgn` are independent of `vmkit`'s local hypervisor: `ix-fleet`
manages **remote** ix VMs (called branches/nodes) over the ix SDK control plane,
and `drgn` is a debugger that runs inside any Linux system (it ships in the base
profile).

## Cross-component invariants

- **One backend per host+guest pair.** Code that boots a VM does not pick a
  hypervisor at runtime; the pair determines it (table above). `vmkit`'s CLI even
  changes shape per host: `boot-linux --disk` exists only on macOS, `--root -- <cmd>`
  only on Linux (`packages/vmkit/src/main.rs:53-103`).
- **macOS VM creation needs an entitled, signed process.** `vmkit` self-signs
  and re-execs (`packages/vmkit/src/main.rs:270-333`) carrying
  `com.apple.security.virtualization` (VZ) and `com.apple.security.hypervisor`
  (libkrun). A Linux host needs no signing (libkrun talks to `/dev/kvm`).
- **Off-screen and cursor-safe by construction.** macOS/Linux GUI capture reads
  the guest framebuffer `IOSurface` directly from an off-screen, non-activating
  window; synthetic input goes straight to the guest's USB devices. Nothing
  appears on the host desktop and the host cursor is never moved.
- **Guest disks build without a VM.** `chrome-vm-image` uses `systemd-repart`
  and `vz-linux-guest` uses `make-disk-image`, so both build on a plain
  aarch64-linux builder with no nested `/dev/kvm`.
- **Platform gating.** The VM packages advertise their flake output and
  package-set attr only on supported systems (`package.nix` in each), so
  `nix flake check` never forces an off-platform build: `vmkit` on
  aarch64-darwin + aarch64/x86_64-linux, `chrome-vm` on aarch64-darwin,
  `chrome-vm-image`/`vz-linux-guest` on aarch64-linux, `drgn` on Linux.

## Glossary

- **VZ / Virtualization.framework**: Apple's host virtualization framework. The
  only macOS-guest backend; also the only working Linux-GUI framebuffer-capture
  path. macOS host only.
- **libkrun**: the Linux-guest hypervisor library. `libkrun-efi` (boots an EFI
  disk via Hypervisor.framework) on a macOS host; classic KVM `libkrun` (boots a
  rootfs under its bundled `libkrunfw` kernel) on a Linux host.
- **Venus / MoltenVK**: libkrun-efi's virtio-gpu device exposes Mesa's "venus"
  Vulkan driver in the guest, backed by MoltenVK (Vulkan-on-Metal) on the host,
  giving a Linux guest a real GPU on Apple Silicon.
- **gvproxy / TSI**: the two guest-networking backends `vmkit` wires: gvproxy
  (gvisor-tap-vsock userspace NAT) on a macOS host, TSI (libkrun transparent
  socket impersonation) on a Linux host.
- **IOSurface**: the shared GPU surface holding the guest framebuffer; read
  directly to PNG without a Screen-Recording grant.
- **bundle**: a macOS-guest directory (`disk.img`, `aux.img`,
  `hardware-model.bin`, `machine-id.bin`) created by `install-macos`.
- **raw EFI disk**: a self-booting disk image carrying its own kernel/bootloader
  (the shape VZ's `VZEFIBootLoader` and libkrun-efi's OVMF both take). What
  `chrome-vm-image` and `vz-linux-guest` produce.
- **fleet plan**: an `ix-fleet` JSON document (`FleetPlan`): an ordered set of
  nodes with their images, switch targets, dependencies, and health checks.
- **node / branch**: an ix control-plane VM. `ix-fleet` calls it a node; the ix
  SDK type is `BranchInfo`/`BranchStatus`.
- **switch**: an in-place NixOS system switch of a running node (vs. a
  delete-then-create image swap, which `ix-fleet` calls `up`/`replace`).
- **drgn**: programmable debugger that walks live struct graphs in a process or
  kernel over `/proc/kcore`; complements `pahole`'s type-layout queries.

## Components

| component | page | what |
| --- | --- | --- |
| vmkit | [vmkit/overview.md](vmkit/overview.md) | own a guest VM lifecycle from Rust; CLI, signing, modules. [linux-guests](vmkit/linux-guests.md) covers the libkrun backend. |
| ix-fleet | [ix-fleet/overview.md](ix-fleet/overview.md) | declarative fleet-plan CLI: create/switch/replace/health remote ix VM nodes via the SDK + dag-runner |
| chrome-vm | [chrome-vm/overview.md](chrome-vm/overview.md) | one-command demo: headless Chromium in a Linux guest under vmkit, screenshot back over the console |
| chrome-vm-image | [chrome-vm-image/overview.md](chrome-vm-image/overview.md) | the aarch64 NixOS guest disk chrome-vm boots (systemd-repart, boot-time screenshot oneshot) |
| vz-linux-guest | [vz-linux-guest/overview.md](vz-linux-guest/overview.md) | the aarch64 NixOS GUI guest disk for vmkit boot-linux-gui (sway + bossbar-overlay on software graphics) |
| drgn | [drgn/overview.md](drgn/overview.md) | programmable live-process/kernel debugger packaged against drgn v0.2.0 |
