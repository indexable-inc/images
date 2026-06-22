# vmkit

`packages/vmkit` owns a guest VM's lifecycle from Rust, with one hypervisor
backend per host and guest OS: macOS guests on Apple's Virtualization.framework
(VZ), Linux guests on libkrun. A single CLI fronts all of it, so other parts of
the system can start and control a VM without holding the macOS entitlements
themselves. The motivating use case: boot and drive a guest fully off-screen and
screenshot its display, so an agent can verify on-screen rendering inside an
isolated VM without the app appearing on the operator's desktop or grabbing the
operator's cursor (`packages/vm/vmkit/src/main.rs:1-24`).

This page covers the CLI surface, the module layout, and the macOS signing
model. The libkrun Linux-guest backend (EFI firmware, GPU/Venus, networking,
capture limits) is in [linux-guests](linux-guests.md).

## Build and flake output

- Rust workspace member (root `Cargo.toml`); binary built by
  `packages/vm/vmkit/default.nix` via `ix.cargoUnit.selectBinaryWithTests`. Flake
  output `vmkit`: `nix run .#vmkit -- info`, `nix build .#vmkit`.
- Platforms (`packages/vm/vmkit/default.nix:9-13`, `package.nix:11-15`):
  `aarch64-darwin`, `aarch64-linux`, `x86_64-linux`. `x86_64-darwin` is omitted
  (libkrun-efi is aarch64-only). The crate compiles everywhere (off-host code is
  `cfg`'d out) but the output is advertised only on supported hosts.
- Dependencies (`packages/vm/vmkit/Cargo.toml`): `clap` + `snafu` are
  unconditional (every host parses args and reports errors). The Apple-framework
  crates (`objc2*`, `block2`, `dispatch2`, `image`, plus `dashboard-core`,
  `tokio`, `base64` for live publishing) are gated to `cfg(target_os = "macos")`,
  so a Linux build pulls none of them (`Cargo.toml:28-61`). The libkrun backend
  is plain FFI with no extra crate; `libkrun` is linked by `build.rs` per host.

## CLI surface

`vmkit` is a `clap` derive CLI (`packages/vm/vmkit/src/main.rs:30-239`). The
`Command` enum is `cfg`-split: only `Info` and `BootLinux` exist on Linux; the
macOS-guest and GUI-capture variants are macOS-only. Dispatch is
`imp::dispatch` on macOS (`src/main.rs:519`) and `dispatch_linux` on Linux
(`src/main.rs:369`).

| subcommand | host | what it does |
| --- | --- | --- |
| `info` | both | Report whether the host can run a VM: `VZVirtualMachine.isSupported()` on macOS (`src/main.rs:686`), `/dev/kvm` present on Linux (`src/main.rs:374`). Prints `virtualization_supported=<bool>`. |
| `boot-linux` | both | Boot a Linux guest under libkrun, stream its serial console, stop on timeout. Guest arg differs by host (below). See [linux-guests](linux-guests.md). |
| `boot-linux-gui` | macOS | Boot an aarch64 Linux GUI guest from a raw EFI disk under VZ, fully off-screen, screenshot the framebuffer to `<out-prefix>.NNN.png` (`src/main.rs:104-129`). |
| `drive-linux` | macOS | Boot a Linux GUI guest off-screen under VZ and drive it from stdin (same protocol as `drive-macos`) (`src/main.rs:130-149`). |
| `install-macos` | macOS | Install macOS into a fresh bundle from a local IPSW via `VZMacOSInstaller`, bypassing Apple's online catalog (`src/main.rs:150-163`). |
| `boot-macos` | macOS | Boot an installed macOS guest off-screen and screenshot its display via the framebuffer `IOSurface`; supports `--share TAG=HOSTDIR` virtio-fs (`src/main.rs:164-184`). |
| `drive-macos` | macOS | Boot a macOS guest off-screen and drive it from stdin: synthetic keyboard/mouse + on-demand screenshots, plus `--share` (`src/main.rs:185-200`). |
| `stage-binary` | macOS | Copy a nix-built binary and rewrite its `/nix/store` dylib refs so it runs on a vanilla guest, then re-sign (`src/main.rs:201-214`). |
| `provision` | macOS | Edit a STOPPED guest bundle's disk to mark Setup Assistant complete (system + per-user), optionally enable auto-login (`src/main.rs:215-238`). |

### `boot-linux` flags (`src/main.rs:53-103`)

- `--disk <raw-efi-disk>` (macOS only): boot a raw EFI disk under libkrun-efi's
  embedded OVMF.
- `--root <dir>` and trailing `-- <cmd>` (Linux only): share a rootfs in over
  virtiofs as `/` and run `<cmd>` as guest init (`exec[0]` is the binary path).
- `--gpu`: attach a virtio-gpu Venus device (`/dev/dri`). On macOS the only way a
  Linux guest gets a GPU.
- `--cpus` (u8, default 2), `--memory-mib` (u32, default 1024): typed to
  libkrun's widths so an out-of-range value is rejected at parse time.
- `--port HOST:GUEST` (repeatable, implies `--net`), `--net`: guest networking
  (gvproxy on macOS, TSI on Linux). Parsed by `build_net` (`src/main.rs:423`).
- `--console-file <path>`: capture the serial console to a file instead of
  stdout (for a background/lockstep caller).
- `--timeout-secs` (default 20): stop after N seconds; `0` runs until the guest
  powers off (persistent-server case).

### `drive-*` command protocol (`src/drive.rs:1-22`)

`drive-macos`/`drive-linux` read newline commands from stdin and ack each on
stdout, so a controller drives the guest in lockstep (capture a frame, locate a
control, click, capture again). Commands: `key <name> [count]`,
`down`/`up <name>` (modifier chords), `type <text>`, `click <fx> <fy>` and
`move`/`scroll` (display fractions, resolution-independent), `cursor`, `size`,
`cursor-show <on|off>`, `wait <seconds>`, `shot <path>`, `quit`. Synthetic
events are delivered straight to the guest's USB keyboard/pointing device by
handing built `NSEvent`s to `VZVirtualMachineView` (`src/input.rs:1-7`), so the
host event system and cursor are never involved.

A `drive-*` session also publishes the live guest screen to the local dashboard
as an image pane (a downscaled PNG sampled ~1/s, unchanged frames dropped),
using `dashboard-core`'s `Publisher` over a producer socket in the discovery
directory. Set `IX_VMKIT_NO_DASHBOARD` to any value to disable it. See
[dashboard-core](../dashboard-core/overview.md).

## Modules (`packages/vm/vmkit/src`)

| module | host | role |
| --- | --- | --- |
| `main.rs` | both | CLI, dispatch, `ensure_signed_and_reexec` (macOS), `build_net`, crate `Error` (`imp::Error`). |
| `linuxkrun.rs` | both | libkrun Linux-guest backend; payload step `cfg`-split per host. See [linux-guests](linux-guests.md). |
| `net.rs` | both | Guest networking: `Net`/`Forward` types (`net.rs:26-36`); gvproxy `Proxy` (macOS) and TSI port map (Linux). |
| `macguest.rs` | macOS | macOS-guest boot, install, off-screen `IOSurface` capture; the off-screen view/capture helpers reused by `linuxguest`. |
| `drive.rs` | macOS | interactive stdin driver + dashboard publishing. |
| `input.rs` | macOS | synthetic `NSEvent` keyboard/key-code map to the guest USB keyboard. |
| `linuxguest.rs` | macOS | Linux GUI guest under VZ (`VZGenericPlatformConfiguration` + `VZEFIBootLoader` + `VZVirtioGraphicsDevice`), reusing `macguest`'s capture path (`linuxguest.rs:1-15`). |
| `provision.rs` | macOS | offline Setup Assistant bypass (host-side disk edit). |
| `stagebin.rs` | macOS | rewrite a Mach-O's `/nix/store` dylib refs to be guest-portable, then re-sign. |

## macOS entitlement self-signing

Creating a VM on macOS requires an entitlement on the *running* process, and the
binary must be code-signed to carry it: `com.apple.security.virtualization` for
the VZ (macOS-guest) paths, `com.apple.security.hypervisor` for the libkrun
(Linux-guest) path. The ix-mcp Python interpreter is an unsigned, immutable Nix
store binary and cannot gain the entitlement, so `vmkit` is a separate signed
binary that owns the VM and callers spawn it.

The signing is automatic (`ensure_signed_and_reexec`, `src/main.rs:269-333`):
any non-`info` command checks the `IX_VMKIT_SIGNED` sentinel; if unset it copies
the read-only store binary into `$XDG_CACHE_HOME/ix/vmkit` (keyed by a hash of
the store path so a rebuild re-signs), ad-hoc-signs the copy with
`src/virtualization.entitlements` (which carries both entitlements plus
`com.apple.security.cs.disable-library-validation` so it can load the libkrun
dylib), and re-execs it with `IX_VMKIT_SIGNED=1`. Concurrent first-runs write to
per-pid temp paths and publish by atomic rename; `prune_stale_signed`
(`src/main.rs:339`) drops copies from prior store paths. On a Linux host no
signing happens (`main` on Linux is `src/main.rs:357`); libkrun talks to
`/dev/kvm` directly. The manual equivalent is `codesign --force --sign -
--entitlements src/virtualization.entitlements <bin>`.

All Virtualization.framework calls run on the process main thread (the queue VZ
binds the VM to); `dispatch_main` drains it so completion handlers fire.

## macOS-guest workflow

The macOS-guest lifecycle is bundle-oriented. A *bundle* is a directory with
`disk.img`, `aux.img`, `hardware-model.bin`, `machine-id.bin`.

1. `install-macos --ipsw <ipsw> --bundle <dir> [--disk-gib 64]` creates the
   bundle from a local restore image via `VZMacOSInstaller`.
2. `provision --bundle <dir> --user <name> [--autologin --password-stdin]`
   marks Setup Assistant complete with the guest STOPPED, so the next boot lands
   on a logged-in desktop. It attaches `disk.img` read-write with no auto-mount,
   finds the synthesized container's APFS Data and System volumes, ensures
   `/var/db/.AppleSetupDone` (system SA), sets per-user `DidSee*` keys plus
   `LastSeenCloudProductVersion`/`LastSeenBuddyBuildVersion` from the System
   volume's `SystemVersion.plist`, and (with `--autologin`) writes `kcpassword`
   (read from stdin, never an argument) and the loginwindow `autoLoginUser`
   (`provision.rs:1-22`). A teardown guard detaches on every exit path and it
   refuses to run on an already-attached image.
3. `boot-macos`/`drive-macos --bundle <dir>` boot off-screen and screenshot or
   drive the guest. `--share TAG=HOSTDIR` shares a host dir over virtio-fs; tag
   `auto` mounts at `/Volumes/My Shared Files` (`parse_shares`, `src/main.rs:658`).
4. `stage-binary <in> <out>` makes a nix-built GUI app guest-portable: for each
   `/nix/store` dylib it repoints to the `/usr/lib` system equivalent when the
   guest ships one (an explicit allowlist, since macOS 11+ serves these from the
   dyld shared cache with no on-disk file) or bundles it next to the output with
   an `@loader_path` reference, recursively over the dependency closure, then
   ad-hoc re-signs and verifies no `/nix/store` path remains (`stagebin.rs:1-24`).

### Off-screen capture (`src/macguest.rs:1-9`)

The guest framebuffer is an `IOSurface` in the `VZVirtualMachineView`'s
framebuffer subview's layer contents; `vmkit` reads its BGRA bytes directly and
encodes PNG with the pure-Rust `image` crate, in-process. The view lives in an
off-screen, non-activating window. This sidesteps the host-side capture paths
that do not work for a headless VM (AppKit `cacheDisplay` reads the Metal layer
black; ScreenCaptureKit needs a per-process Screen-Recording grant;
`screencapture -l` cannot capture a fully off-screen window), so no
Screen-Recording permission is needed.

## MCP tool provider

`vmkit` is also exposed as an ix-mcp tool provider: a `vmkit` Python module
bundled into the pinned interpreter wraps the binary (`info`, `install`,
`provision`, `stage_binary`, `screenshot`, `screenshot_many`, `drive`,
`Driver`, `run_app`, plus Linux `boot_linux`/`boot_linux_gui`/`drive_linux`),
returning PIL images that render inline. That module is owned by the
[mcp](../mcp/tool-providers/overview.md) domain; its internals are not documented here.

## Not yet built

A vsock control channel and a long-lived `serve` mode for IPC are designed but
not implemented (`packages/vm/vmkit/README.md` roadmap). Today each guest is an
independent `vmkit` process spawned per operation.
