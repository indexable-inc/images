# vmkit

Own a virtual machine's lifecycle from Rust, with one hypervisor backend per host
and guest OS:

- **macOS guests** (macOS host) run on Apple's [Virtualization.framework](https://developer.apple.com/documentation/virtualization)
  (via [`objc2-virtualization`](https://docs.rs/objc2-virtualization)): boot an
  installed macOS, drive it off-screen, screenshot its framebuffer.
- **Linux guests** run on [libkrun](https://github.com/containers/libkrun): on a
  macOS host the EFI / Hypervisor.framework variant (the only backend that gives a
  Linux guest GPU acceleration on Apple Silicon), and on a Linux host classic KVM
  libkrun (the bundled kernel boots a rootfs + exec command, the same model
  `podman --runtime krun` uses). See [`docs/linux-libkrun.md`](docs/linux-libkrun.md).

A small CLI fronts all of these, so other parts of the system can start and
control a VM without holding the entitlements themselves.

The motivating macOS use case: run a GUI app (for example the `bossbar-overlay`)
inside a VM and inspect it remotely, so an agent can verify on-screen rendering
without the app ever appearing on the operator's real desktop or grabbing the
operator's cursor.

```sh
nix run .#vmkit -- info

# macOS host: boot a Linux guest from a raw EFI disk under libkrun-efi, with a GPU:
nix run .#vmkit -- boot-linux --disk ./linux.raw --gpu

# Linux host: boot a Linux guest from a rootfs dir under classic libkrun (KVM),
# running a command as the guest init:
nix run .#vmkit -- boot-linux --root ./rootfs -- /bin/busybox sh -c 'uname -a; ls /'
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

- `info` reports whether the host can run a VM (`VZVirtualMachine.isSupported` on
  macOS, `/dev/kvm` present on Linux).
- `boot-linux` boots a Linux guest **via libkrun** and streams its serial console,
  then stops on a timeout. The guest argument differs by host:
  - **macOS host**: `--disk <raw-efi-disk>` boots the disk under libkrun-efi's
    embedded OVMF (not VZ). `--gpu` adds a virtio-gpu Venus device
    (`/dev/dri/renderD128`, Vulkan via MoltenVK), which VZ cannot give a Linux
    guest.
  - **Linux host**: `--root <rootfs-dir>` boots that directory (shared in over
    virtiofs) under classic libkrun's bundled KVM kernel, running the trailing
    command as the guest init, e.g. `boot-linux --root ./rootfs -- /bin/ls /`.
    No `/dev/kvm`, no boot.

  See [`docs/linux-libkrun.md`](docs/linux-libkrun.md).
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
  re-execs an ad-hoc-signed copy from `$XDG_CACHE_HOME/ix/vmkit` carrying the
  `com.apple.security.virtualization` entitlement, so `nix run .#vmkit` and
  ix-mcp spawning work with no manual `codesign` step.

The `vmkit` Python module bundled into ix-mcp exposes the full surface:
`info`, `install`, `provision`, `stage_binary`, `screenshot`,
`screenshot_many`, `drive`, `Driver`, and the one-call `run_app` (share a host
app in, launch it, return a frame of the guest display). For Linux guests it
adds `boot_linux` (boot a raw EFI disk headlessly under libkrun, `gpu=True` for a
virtio-gpu device, return the serial console as a string), `boot_linux_gui` (boot
a raw EFI disk off-screen under VZ, return a `PIL.Image` of the render),
`drive_linux`, and `Driver(disk=...)`.

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

A `drive-macos` or `drive-linux` session also publishes the guest screen to the
local dashboard automatically, the same way a terminal producer does: it binds a
producer socket in the [discovery directory](../dashboard-core) and streams the
framebuffer as a live image pane. Run the standalone `dashboard` aggregator and
the running guest appears on the canvas next to any terminals, with no extra
flag.

The pane is a downscaled PNG (capped at 900px wide, aspect preserved) sampled
about once a second; an unchanged frame is dropped before encoding, so an idle
desktop publishes one frame and then nothing. The raw frame is copied off the
`IOSurface` on the main queue and converted, scaled, and compared off it, to keep
guest rendering and lockstep input responsive. The capture is best-effort: if the
socket cannot be bound the driver logs one line and keeps running. Set
`IX_VMKIT_NO_DASHBOARD` (to any value) to turn it off, e.g. a lockstep automated
driver that does not want the extra framebuffer sampling.

Known limit: the dashboard keeps pane history in a CRDT, so a screen that changes
continuously for a very long session grows the aggregator's in-memory state. For
the interactive, mostly-static desktops this targets it stays small; a smaller
cap or lower rate bounds it further.

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
nix run .#vmkit -- install-macos --ipsw ./UniversalMac_26.5_Restore.ipsw --bundle ./guest
# --autologin reads the password from stdin (keeps it out of the process table);
# omit --password-stdin for an empty password.
printf '%s' "$PASSWORD" | nix run .#vmkit -- provision --bundle ./guest --user ix --autologin --password-stdin
nix run .#vmkit -- drive-macos --bundle ./guest   # lands on the desktop, no Setup Assistant
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
nix run .#vmkit -- stage-binary ./result/bin/myapp ./staged/myapp
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
- Instead, `vmkit` is a separate signed binary that owns the VM. Callers
  (the CLI, and later the `vmkit` Python module) spawn it and talk to it over a
  control channel. The entitlement lives only on this process.

This is the same split [`go-microvm`](https://github.com/stacklok/go-microvm)
and [`ericcurtin/vmm`](https://github.com/ericcurtin/vmm) use: a small signed
runner that the rest of the program drives.

### Signing

Ad-hoc signing is enough; no paid Developer ID is required. This is automatic:
a VM command checks for a sentinel env var, and if unset copies the (read-only)
Nix store binary into `$XDG_CACHE_HOME/ix/vmkit` (keyed by the store path),
ad-hoc-signs it with `src/virtualization.entitlements`
(`com.apple.security.virtualization`), and re-execs it. So `nix run .#vmkit`
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

For a Linux GUI guest the off-screen-capture path above still uses VZ, but VZ
gives Linux guests no 3D acceleration: `wgpu` falls back to software (lavapipe).
GPU-accelerated Linux rendering is exactly why the **headless** Linux path uses
libkrun instead (next section).

## Linux guests: libkrun

Linux guests boot on [libkrun](https://github.com/containers/libkrun), not
Virtualization.framework, because libkrun is the only backend that gives a Linux
guest a real GPU on Apple Silicon: its macOS variant (`libkrun-efi`) ships a
Venus virtio-gpu device backed by MoltenVK, so the guest gets Vulkan and a
`/dev/dri/renderD128` node. `boot-linux --disk <raw-efi-disk> --gpu` boots an
EFI-bootable disk and streams its serial console. This is the same conclusion
Podman Desktop, Lima, and colima reached (they use libkrun/krunkit on macOS).

Details (the EFI-variant constraint, the embedded OVMF firmware, linking, and the
`com.apple.security.hypervisor` entitlement) are in
[`docs/linux-libkrun.md`](docs/linux-libkrun.md). The off-screen **GUI** capture
paths (`boot-linux-gui`, `drive-linux`) stay on VZ: libkrun-efi exposes a display
backend API (`krun_add_display` + `krun_set_display_backend`), but it does not
actually capture a frame on macOS with the current libkrun + nixpkgs venus-only
`virglrenderer` build. Its scanout readback is a virgl **GL** `glReadPixels`, and
this build has no macOS GL backend (venus does not bypass it: `SET_SCANOUT_BLOB`
is unimplemented). Capturing a guest framebuffer via libkrun on macOS needs the
upstream Metal-texture scanout work (virglrenderer `create_handle_for_scanout` +
libkrun `SET_SCANOUT_BLOB`), so VZ remains the only working Linux-GUI capture path
here. See [`docs/linux-libkrun.md`](docs/linux-libkrun.md).

## Build notes

- This is the first workspace crate that links an Apple framework. The objc2
  dependencies are gated to `cfg(target_os = "macos")` so the Linux CI workspace
  graph never pulls them; on Linux the binary compiles as a typed "macOS only"
  stub. The package output is advertised only on `aarch64-darwin`.
- All Virtualization.framework calls happen on the process main thread (the
  queue VZ binds the VM to by default); `dispatch_main` drains that queue so
  completion handlers fire, mirroring Apple's sample app.

## Bad fit if

- You need an off-screen **GUI** capture of a GPU-accelerated Linux desktop: the
  headless Linux path has a GPU (libkrun), but the off-screen framebuffer-capture
  paths (`boot-linux-gui`, `drive-linux`) are still VZ, which has no Linux GPU.
- You cannot code-sign: without the virtualization/hypervisor entitlements,
  VM creation fails by design.
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
6. ~~`vmkit` Python module bundled into ix-mcp (like `tui`/`screen`).~~ Done: the
   module exposes `info`, `install`, `provision`, `stage_binary`, `screenshot`,
   `screenshot_many`, `drive`, `Driver`, and `run_app`, returning PIL images that
   render inline, plus the Linux helpers `boot_linux` (headless serial console),
   `boot_linux_gui`, and `drive_linux`.
7. ~~Linux guests on libkrun (GPU via Venus/MoltenVK).~~ Done: `boot-linux
   --disk [--gpu]` boots a raw EFI disk under libkrun and streams its console;
   see [`docs/linux-libkrun.md`](docs/linux-libkrun.md).
8. Linux-GUI off-screen capture on libkrun is **blocked upstream**, not just
   unimplemented here: libkrun's only scanout-readback path is a virgl GL
   `glReadPixels`, which has no macOS GL backend in the venus-only build, and
   `SET_SCANOUT_BLOB` is unimplemented, so neither a venus guest nor a flag makes
   `present_frame` fire. It needs the Metal-texture scanout work (virglrenderer
   `create_handle_for_scanout` + libkrun `SET_SCANOUT_BLOB`; UTM venus-on-macOS,
   Dec 2025). Until then GUI capture stays on VZ.
