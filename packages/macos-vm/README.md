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

Proven end to end on macOS 26.5 / Apple M5 Max. The headline run: a signed
binary boots an installed macOS guest fully off-screen, an operator drives it
through Setup Assistant to a logged-in desktop with synthetic keyboard and mouse
input (no host cursor), shares a host directory in over virtio-fs, launches a
wgpu GUI app (the `bossbar-overlay`) from that share, and screenshots the running
overlay by reading the guest framebuffer. Nothing appears on the host desktop and
the host cursor is never touched. A Linux guest also boots to userspace with its
serial console streamed to stdout.

What works today:

- `info` reports `VZVirtualMachine.isSupported`.
- `boot-linux` boots a Linux guest and streams its console, then stops on a
  timeout.
- `boot-linux-gui` boots an aarch64 **Linux GUI** guest from a raw EFI disk with
  a virtio-gpu display + USB keyboard/mouse, fully off-screen, and screenshots
  the guest framebuffer to PNGs (same IOSurface capture as `boot-macos`). The
  `vz-linux-guest` package builds a NixOS disk that boots straight into a sway
  compositor running a GUI app on software graphics (Mesa lavapipe), so a wgpu
  app like `bossbar-overlay` is verified rendering on Linux without a GPU.
- `drive-linux` drives a Linux GUI guest from stdin, same command protocol as
  `drive-macos`.
- `install-macos` installs macOS into a fresh bundle from a local restore image
  (IPSW), via `VZMacOSInstaller`. Bypasses Apple's online catalog (gdmf), which
  is TLS-intercepted on some networks: download the `.ipsw` and pass it.
- `boot-macos` boots an installed macOS guest **fully off-screen** and
  screenshots its display to PNGs, with no visible window, no
  ScreenCaptureKit, and no Screen-Recording permission (see
  [Off-screen capture](#off-screen-capture)).
- `drive-macos` boots the guest off-screen and reads newline commands from
  stdin to drive it: synthetic keyboard (`key`/`down`/`up`/`type`), mouse
  (`click`), `wait`, and on-demand `shot`, with a one-line ack per command (see
  [Driving the guest](#driving-the-guest)). Input goes straight to the guest's
  keyboard/pointing device, so the host cursor never moves.
- **virtio-fs directory sharing**: `--share TAG=HOSTDIR` on `boot-macos`
  and `drive-macos` shares a host directory into the guest. Tag `auto` uses the
  macOS automount tag, mounting at `/Volumes/My Shared Files`.
- `provision` marks a freshly installed (stopped) guest's Setup Assistant
  complete by editing its disk from the host, so the next boot lands on a
  logged-in desktop with no Setup Assistant clicks (see
  [Provisioning past Setup Assistant](#provisioning-past-setup-assistant)).
- `stage-binary` copies a nix-built macOS binary and rewrites its `/nix/store`
  dylib references so it runs on a vanilla guest (see
  [Staging a binary guest-portable](#staging-a-binary-guest-portable)).
- **Automatic self-signing**: a VM command on the read-only Nix store binary
  re-execs an ad-hoc-signed copy from `$XDG_CACHE_HOME/ix/macos-vm` carrying the
  `com.apple.security.virtualization` entitlement, so `nix run .#macos-vm` and
  ix-mcp spawning work with no manual `codesign` step.

The `macvm` Python module bundled into ix-mcp exposes the full surface:
`info`, `install`, `provision`, `stage_binary`, `screenshot`,
`screenshot_many`, `drive`, `Driver`, and the one-call `run_app` (share a host
app in, launch it, return a frame of the guest display). For Linux guests it
adds `boot_linux` (boot a raw kernel `Image` + initramfs headlessly, attach OCI
rootfs disks via `disks=[...]`, return the serial console as a string),
`boot_linux_gui` (boot a raw EFI disk off-screen, return a `PIL.Image` of the
render), `drive_linux`, and `Driver(disk=...)`.

What is designed but not yet built (see [Roadmap](#roadmap)):

- a vsock control channel and a long-lived `serve` mode.

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

## Driving the guest

`drive-macos` boots the guest off-screen and then reads commands from stdin, one
per line, acking each on stdout. Synthetic events are delivered straight to the
guest's USB keyboard and screen-coordinate pointing device by handing built
`NSEvent`s to the `VZVirtualMachineView` (the technique
[thecrypticace/vzautomation](https://github.com/thecrypticace/vzautomation)
uses), so the host event system and cursor are never involved.

Commands:

- `key <name> [count]` press a named key (`return`, `tab`, `space`, `esc`,
  arrows, `delete`, `f1`..`f12`, modifiers) `count` times
- `down <name>` / `up <name>` hold / release a key, e.g. to chord a modifier
- `type <text>` type the rest of the line (US-layout printable ASCII)
- `click <fx> <fy>` left-click at a fraction `0..1` of the display, from the
  top-left (resolution-independent, so it survives display-mode changes)
- `wait <seconds>` sleep; `shot <path>` screenshot the framebuffer; `quit` exit

Because every command acks, a controller can drive the guest in lockstep:
capture a frame, locate a control in it (the host side can use any image
tooling), `click` it, capture again. Modifiers via `down`/`up` give chords like
Spotlight: `down cmd`, `key space`, `up cmd`.

Modal screens that make a network call (Apple ID, Screen Time) hang on a host
without working guest internet. Where the goal is just a usable desktop, it is
simpler to mark Setup Assistant complete offline by editing the guest disk
(`.AppleSetupDone` plus the per-user `com.apple.SetupAssistant` `DidSee*` keys)
than to click through those screens. The `provision` subcommand does exactly
that edit; see the next section.

## Provisioning past Setup Assistant

A freshly installed guest boots into Setup Assistant. On a host whose content
filter breaks the guest's TLS to Apple, the network-backed screens (Apple ID,
Screen Time) hang forever with greyed buttons, so the guest cannot be clicked
past them offline. `provision` performs the proven host-side disk edit, with the
guest stopped, so the next boot lands on a logged-in desktop:

```sh
nix run .#macos-vm -- install-macos --ipsw ./UniversalMac_26.5_Restore.ipsw --bundle ./guest
# --autologin reads the password from stdin (keeps it out of the process table);
# omit --password-stdin for an empty password.
printf '%s' "$PASSWORD" | nix run .#macos-vm -- provision --bundle ./guest --user ix --autologin --password-stdin
nix run .#macos-vm -- drive-macos --bundle ./guest   # lands on the desktop, no Setup Assistant
```

It attaches the bundle's `disk.img` read-write with no auto-mount, finds the
synthesized container's APFS **Data** and **System** volumes, and:

- ensures `private/var/db/.AppleSetupDone` exists on the Data volume (gates the
  system Setup Assistant: language, country, account creation);
- sets every per-user `DidSee*` key true in
  `Users/<user>/Library/Preferences/com.apple.SetupAssistant.plist` (gates the
  per-user MiniBuddy flow: Apple ID, Screen Time, Siri, appearance, …) and sets
  `LastSeenCloudProductVersion`/`LastSeenBuddyBuildVersion` to the guest OS
  version read from the System volume's `SystemVersion.plist`, so the cloud
  screen does not re-prompt;
- with `--autologin`, writes `/private/etc/kcpassword` (the password read from
  stdin via `--password-stdin`, never an argument) and the loginwindow
  `autoLoginUser` so the guest boots straight to the desktop with no password.

It reads the OS version from the guest **System** volume directly: the System
firmlinks do not resolve when the Data volume is mounted standalone from the
host, so the Data and System volumes are mounted and matched within the one
container backed by the attached image (a host System volume in
`diskutil apfs list` is never touched). A teardown guard unmounts and detaches
on every exit path, so a failure part-way never leaves the image attached, and
it refuses to run if the image is already attached (editing a live filesystem
would corrupt it).

## Staging a binary guest-portable

A nix-built macOS binary links its dylibs by absolute `/nix/store` path, which a
vanilla guest does not have, so it fails to start when shared in. `stage-binary`
copies the binary and makes the copy depend only on libraries the guest has:

```sh
nix run .#macos-vm -- stage-binary ./result/bin/myapp ./staged/myapp
otool -L ./staged/myapp    # zero /nix/store entries
```

For each `/nix/store` dylib in `otool -L`, it repoints to the `/usr/lib` system
equivalent when the guest ships one (libiconv, libc++, libresolv, libobjc, …) or,
when it does not, copies the dylib next to the output and rewrites the reference
to `@loader_path/<name>`. A bundled dylib is then staged in turn: its own install
id (`LC_ID_DYLIB`) is rewritten to `@loader_path/<name>` with `install_name_tool
-id`, and its own `/nix/store` load deps are repointed or bundled the same way,
recursively, so the whole dependency closure is guest-portable, not just the
top-level binary. Every artifact (the output and each bundled dylib) is ad-hoc
re-signed, since a Mach-O whose load commands changed needs a fresh signature,
and each is checked for a remaining `/nix/store` path, erroring otherwise (no
silent partial result). The common system libraries are an explicit allowlist:
macOS 11+ serves them from the dyld shared cache, so they have no on-disk file
and a naive `exists("/usr/lib/libiconv.2.dylib")` check returns false even though
the library loads.

`run_app` (Python) wires this together: stage an app into a directory, `--share`
it in, and launch it in the guest in one call.

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

Ad-hoc signing is enough; no paid Developer ID is required. This is automatic:
a VM command checks for a sentinel env var, and if unset copies the (read-only)
Nix store binary into `$XDG_CACHE_HOME/ix/macos-vm` (keyed by the store path),
ad-hoc-signs it with `src/virtualization.entitlements`
(`com.apple.security.virtualization`), and re-execs it. So `nix run .#macos-vm`
and ix-mcp spawning work with no manual `codesign`. The equivalent manual step
is `codesign --force --sign - --entitlements src/virtualization.entitlements <bin>`.

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

The first option above is proven minimally: `boot-linux --disk` attaches a
flattened OCI rootfs as a virtio-blk disk, and `examples/oci-boot.sh` boots a
busybox OCI image to userspace over the serial console. See
[`docs/oci-guest.md`](docs/oci-guest.md) for the build-vs-delegate decision, the
proof, and its gaps (squashfs not ext4, externally fetched kernel).

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
2. ~~Rust `install-macos` from a local IPSW.~~ Done.
3. ~~First-run entitlement self-signer.~~ Done (see [Signing](#signing)).
4. ~~Guest input injection to drive the guest headlessly without the host
   cursor.~~ Done: `drive-macos` (see [Driving the guest](#driving-the-guest)).
   Getting past Setup Assistant is done offline by `provision` (see
   [Provisioning past Setup Assistant](#provisioning-past-setup-assistant))
   rather than by driving the network-backed screens; `stage-binary` plus
   `run_app` cover launching a nix-built app. Vision OCR for locating controls
   from a frame is still open.
5. vsock control channel + long-lived `serve` mode for IPC.
6. ~~`macvm` Python module bundled into ix-mcp (like `tui`/`screen`).~~ Done: the
   module exposes `info`, `install`, `provision`, `stage_binary`, `screenshot`,
   `screenshot_many`, `drive`, `Driver`, and `run_app`, returning PIL images that
   render inline, plus the Linux helpers `boot_linux` (headless serial console),
   `boot_linux_gui`, and `drive_linux`.
7. ~~OCI-disk boot for Linux guests; document the libkrun handoff for
   microVMs.~~ Done minimally: `boot-linux --disk` + `examples/oci-boot.sh` boot
   a flattened OCI rootfs as a virtio-blk disk; build-vs-delegate decision and
   gaps in [`docs/oci-guest.md`](docs/oci-guest.md).
