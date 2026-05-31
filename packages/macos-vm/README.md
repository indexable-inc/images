# macos-vm

Drive Apple's [Virtualization.framework](https://developer.apple.com/documentation/virtualization)
from Rust. This crate is a thin binding over
[`objc2-virtualization`](https://docs.rs/objc2-virtualization) plus a small CLI
that owns a virtual machine's lifecycle, so other parts of the system can start
and control a VM without holding the virtualization entitlement themselves.

The motivating use case: run a GUI app (for example the `bossbar-overlay`) inside
a VM and inspect it remotely, so an agent can verify on-screen rendering without
the app ever appearing on the operator's real desktop or grabbing the operator's
cursor.

```sh
nix run .#macos-vm -- info
nix run .#macos-vm -- boot-linux --kernel ./Image --initramfs ./initramfs
```

## Status

Proven end to end on macOS 26.5 / Apple M5 Max: a signed binary boots a real
Linux guest (raw arm64 kernel `Image` + initramfs) through Virtualization.framework
and streams the guest serial console to stdout. The guest reaches userspace.

What works today:

- `info` reports `VZVirtualMachine.isSupported`.
- `boot-linux` boots a Linux guest and streams its console, then stops on a
  timeout.
- `boot-macos` boots an installed macOS guest **fully off-screen** and
  screenshots its display to PNGs, with no visible window, no
  ScreenCaptureKit, and no Screen-Recording permission (see
  [Off-screen capture](#off-screen-capture)). Validated end to end: captures the
  macOS Setup Assistant from a guest whose window is parked at (-20000, -20000).

What is designed but not yet built (see [Roadmap](#roadmap)):

- a Rust `install-macos` (today the bundle is produced by a reference Swift tool),
- a vsock control channel and a long-lived `serve` mode,
- booting an OCI image as a disk,
- guest input injection + OCR to drive the guest headlessly (e.g. past Setup
  Assistant) without the host cursor,
- the `macvm` Python module bundled into ix-mcp.

## Off-screen capture

The guest framebuffer is an `IOSurface` living in the `VZVirtualMachineView`'s
framebuffer subview's layer contents:

```rust
let surface = vm_view.subviews().firstObject()?.layer()?.contents(); // IOSurface
```

`boot-macos` reads that IOSurface's BGRA bytes directly and encodes a PNG with
the pure-Rust [`image`](https://docs.rs/image) crate, entirely in-process. The
view lives in an off-screen, non-activating window, so nothing appears on the
host desktop and the cursor is never captured.

This matters because the host-side capture paths do **not** work for a headless
VM: the VZ display is a Metal-backed layer, so AppKit's `cacheDisplay` reads it
black; ScreenCaptureKit needs a per-process Screen-Recording grant (a fresh
helper gets `SCStreamError -3811`); and `screencapture -l <windowID>` cannot
capture a fully off-screen window ("could not create image from window"). The
IOSurface read sidesteps all of that. Technique from
[thecrypticace/vzautomation](https://github.com/thecrypticace/vzautomation),
which also shows keyboard injection and Vision OCR for driving the guest.

## Why a standalone signed binary

Creating a VM requires the `com.apple.security.virtualization` entitlement on the
**running process**, and the binary must be code-signed to carry it. That shapes
the architecture:

- The ix-mcp Python interpreter is an unsigned, immutable Nix store binary. It
  cannot gain the entitlement, and re-signing a store path is not an option. So
  the interpreter must not drive Virtualization.framework in-process.
- Instead, `macos-vm` is a separate signed binary that owns the VM. Callers
  (the CLI, and later the `macvm` Python module) spawn it and talk to it over a
  control channel. The entitlement lives only on this process.

This is the same split [`go-microvm`](https://github.com/stacklok/go-microvm)
and [`ericcurtin/vmm`](https://github.com/ericcurtin/vmm) use: a small signed
runner that the rest of the program drives.

### Signing

Ad-hoc signing is enough; no paid Developer ID is required:

```sh
codesign --force --sign - --entitlements virtualization.entitlements macos-vm
```

with an entitlements plist containing `com.apple.security.virtualization`. Nix
store outputs are read-only, so the package signs into a per-user cache on first
run and re-execs the signed copy. (Wiring this into `default.nix` is on the
roadmap; today the e2e signs the built binary out-of-store.)

## Visual testing without taking over the host

The `bossbar-overlay` is a native `winit` + `wgpu` (Metal) app: transparent,
always-on-top, click-through windows that float over the desktop, with
hover-to-grow and native drag. Its static rendering is already verifiable
headless via `bossbar-overlay --snapshot out.png`. What cannot be checked
without taking over the real screen is the **live windowed behavior**:
always-on-top placement, the native drag, click-through, menu-bar clearance,
multi-window stacking.

That behavior is faithful only on a macOS desktop, so the test environment is a
**macOS guest VM**, which gets GPU-accelerated graphics through
`VZMacGraphicsDevice` (so Metal and `wgpu` render for real). Two ways to see and
drive the guest, neither of which touches the host cursor or desktop:

1. **Remote access into the guest.** Enable Screen Sharing (VNC) inside the
   macOS guest and connect from the host. Input over VNC drives the *guest*
   pointer, so the host cursor is never captured. This matches "use remote
   access in it to test it visually" directly.
2. **Offscreen framebuffer capture.** Attach a `VZVirtualMachineView` to an
   off-screen, non-activating window and render it to an `NSBitmapImageRep`. The
   host shows no visible window. This is the lighter path for a single
   screenshot; it needs validation that VZ renders an off-screen view (tracked
   in the roadmap).

For a Linux guest the same applies, except Virtualization.framework gives Linux
guests no 3D acceleration: `wgpu` would fall back to software (lavapipe). For
GPU-accelerated Linux rendering on macOS you need a different VMM (see OCI
below), not Virtualization.framework.

## OCI images

Two honest options:

- **Virtualization.framework + a disk built from an OCI image.** Flatten the
  image into a raw/ext4 disk (the repo's `oci-image-builder` can do the
  flattening) and boot it with `VZLinuxBootLoader` or `VZEFIBootLoader`. Pure
  Virtualization.framework, no extra dependency, but you manage kernel/initrd and
  get no GPU acceleration in the Linux guest.
- **[libkrun](https://github.com/containers/libkrun) / krunkit.** Purpose-built
  to boot an OCI image as a microVM (the image is the rootfs over virtio-fs,
  boot in milliseconds), and `libkrun-efi` adds a Venus virtio-gpu so Linux
  guests get real Vulkan via MoltenVK. It uses Hypervisor.framework and needs
  the `com.apple.security.hypervisor` entitlement.

Recommended default: use Virtualization.framework for the macOS-guest visual
test (the actual goal here), and reach for libkrun/krunkit for OCI microVMs
rather than reimplementing OCI-on-Virtualization.framework. libkrun already owns
that surface, including the GPU path, and the maintenance cost of rebuilding it
is not justified yet. If a single backend ever becomes a hard requirement, that
decision should be made deliberately, not by accretion.

## Build notes

- This is the first workspace crate that links an Apple framework. The objc2
  dependencies are gated to `cfg(target_os = "macos")` so the Linux CI workspace
  graph never pulls them; on Linux the binary compiles as a typed "macOS only"
  stub. The package output is advertised only on `aarch64-darwin`.
- All Virtualization.framework calls happen on the process main thread (the
  queue VZ binds the VM to by default); `dispatch_main` drains that queue so
  completion handlers fire, mirroring Apple's sample app.

## Bad fit if

- You need GPU-accelerated **Linux** rendering on macOS: Virtualization.framework
  does not accelerate Linux guests; use libkrun-efi/krunkit instead.
- You cannot code-sign: without the virtualization entitlement, configuration
  validation fails by design.
- You are off Apple Silicon: only `aarch64-darwin` is wired up.

## Roadmap

1. ~~Graphics device + off-screen screenshot.~~ Done: `boot-macos` (see
   [Off-screen capture](#off-screen-capture)).
2. Rust `install-macos` (`VZMacOSInstaller` from a local IPSW) so the package
   creates its own guest bundle. Today a reference Swift tool does it; the
   Apple restore catalog (gdmf) is fetched via mesu + the CDN because gdmf is
   TLS-intercepted on some networks.
3. Guest input injection + Vision OCR to drive the guest headlessly (past Setup
   Assistant, then launch an app) without the host cursor.
4. vsock control channel + long-lived `serve` mode for IPC.
5. `macvm` Python module bundled into ix-mcp (like `tui`/`screen`): spawns this
   signed binary and exposes `boot`, `screenshot`, and input, returning PIL
   images that render inline.
6. OCI-disk boot for Linux guests; document the libkrun handoff for microVMs.
7. Nix-integrated first-run entitlement signer in `default.nix`.
