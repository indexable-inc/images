# vmkit: Linux guests on libkrun

Linux guests boot on [libkrun](https://github.com/containers/libkrun), not
Virtualization.framework, with a different libkrun per host
(`packages/vm/vmkit/src/linuxkrun.rs:1-21`). The macOS split is deliberate: VZ
gives a Linux guest no GPU, while libkrun-efi ships a virtio-gpu Venus device
backed by MoltenVK, so a Linux guest gets real Vulkan on Apple Silicon. The two
boot models share the ctx/config/gpu/console/watchdog/`krun_start_enter`
skeleton in `linuxkrun.rs`; only the payload step is `cfg`-split. See the
[overview](overview.md) for the CLI flags and the source of truth at
`packages/vm/vmkit/docs/linux-libkrun.md`.

## The two boot models

The `BootLinux` params struct (`linuxkrun.rs:144-179`) carries the per-host
payload (`disk` on macOS, `root` + `exec` on Linux) plus shared fields
(`gpu`, `cpus`, `memory_mib`, `net`, `console_file`, `timeout`).

### macOS host: EFI disk under libkrun-efi

`vmkit boot-linux --disk <raw-efi-disk> [--gpu]` boots a raw EFI-bootable disk
(a NixOS `raw-efi` image, Fedora CoreOS raw, etc.) and streams its serial
console. nixpkgs only provides libkrun on Darwin as `libkrun-efi` (classic
libkrun's `libkrunfw` kernel package does not build on macOS). The EFI build
always boots its embedded OVMF/EDK2 firmware, so `krun_set_kernel`/`krun_set_root`
are ignored and a guest must be an EFI-bootable disk carrying its own
kernel/bootloader (the same disk shape `VZEFIBootLoader` takes).

Call sequence: `krun_create_ctx` -> `krun_set_vm_config` -> `krun_set_firmware`
-> `krun_add_disk2` -> (optional) `krun_set_gpu_options2` -> (optional)
`krun_set_console_output` -> `krun_start_enter`.

The OVMF blob (`KRUN_EFI.silent.fd`, ~2 MiB) is embedded into the binary at
build time (`KRUN_EFI_FIRMWARE`, `linuxkrun.rs:142`) and written to a per-user
cache file at runtime because `krun_set_firmware` wants a path
(`firmware_path`, `linuxkrun.rs:188`); embedding keeps the binary self-contained
across the entitlement self-sign re-exec.

### Linux host: rootfs under classic KVM libkrun

`vmkit boot-linux --root <rootfs-dir> [--gpu] -- <cmd> [args...]` shares a host
directory into the guest over virtiofs as `/`, boots it under libkrun's bundled
`libkrunfw` kernel, and runs `<cmd>` as the guest init. Same model
`podman --runtime krun` / `crun` use: no firmware, no guest-supplied kernel, no
EFI disk. Needs `/dev/kvm`.

Call sequence: `krun_create_ctx` -> `krun_set_vm_config` -> `krun_set_root` ->
`krun_set_workdir` -> `krun_set_exec` -> (optional) `krun_set_gpu_options2` ->
(optional) `krun_set_console_output` -> `krun_start_enter`.

`krun_set_exec(exec_path, argv, envp)` uses `exec_path` as the guest's `argv[0]`,
so `vmkit` passes `exec[0]` as `exec_path` and `exec[1..]` as `argv`; passing the
full vector would duplicate `argv[0]` (e.g. `/bin/sh /bin/sh -c ...`).

`krun_start_enter` does not return on success on either host: libkrun takes over
the process and `exit()`s with the guest's exit code when the VM stops, so the
console has streamed by then. A watchdog thread bounds the run so a background
invocation never hangs (`--timeout-secs`; `0` disables it for a persistent
server).

## GPU (`--gpu`)

`--gpu` attaches a virtio-gpu Venus device; rendering on it also needs guest
drivers. The guest kernel must load `virtio_gpu` (creates `/dev/dri/renderD128`)
and guest userspace needs Mesa's venus Vulkan driver (nixpkgs builds it into
`mesa` as `virtio_icd.aarch64.json`). When the stack works,
`vulkaninfo --summary` reports a `Virtio-GPU Venus` device; a lavapipe/llvmpipe
device means a software fallback (the venus ICD was not loaded). The known trap
is an old guest Mesa (venus against MoltenVK needed Mesa fixes that some
stable-distro Mesas lacked); a current Mesa, including a stock NixOS guest from
this repo's pin, has them.

## Guest networking (`src/net.rs`)

libkrun's default backend is TSI (transparent socket impersonation), which needs
a TSI-aware guest kernel: the bundled `libkrunfw` kernel (Linux host) has it, a
stock NixOS kernel from an EFI disk (macOS host) does not. So `vmkit` wires
networking two ways behind `--net` / `--port HOST:GUEST` (`net.rs:1-23`,
`Net`/`Forward` at `net.rs:26-36`):

- **Linux host** (classic libkrun + libkrunfw): TSI. Outbound works with no
  setup; inbound host->guest ports use `krun_set_port_map` (a list of
  `"host:guest"` strings).
- **macOS host** (libkrun-efi + stock guest kernel): `gvproxy`
  (gvisor-tap-vsock), the same proxy krunkit/podman-machine use. `vmkit` spawns
  gvproxy on a temp unix socket, attaches the guest NIC with
  `krun_set_gvproxy_path`, and POSTs each forward to gvproxy's HTTP control API
  (`/services/forwarder/expose`). `krun_set_port_map` is TSI-only (`-ENOTSUP`
  under a proxy). gvproxy is resolved from `IX_VMKIT_GVPROXY` (a Nix store path)
  or `gvproxy` on `PATH`.

gvproxy puts the guest on `192.168.127.0/24` (gateway `.1`, guest `.2` via
DHCP), so a macOS-host guest image must DHCP its NIC. `--port 3200:3200` makes
the guest's `:3200` reachable on the host's `:3200`, bound on all host
interfaces. Combined with `--timeout-secs 0`, this is the persistent-server case
(e.g. hosting a service in a NixOS EFI guest).

## Linking and entitlements

The crate links `-lkrun`, resolved per host by the Nix build: `libkrun-efi.dylib`
on macOS (`${libkrun-efi}/lib`), classic `libkrun.so` on Linux
(`${libkrun}/lib64`). `build.rs` emits the `-l` (gated on the `have_libkrun`
cfg, set when libkrun is available for the build host); the search path and
rpath are injected by the workspace build (`lib/rust/workspace.nix`). On Linux,
nixpkgs `libkrun` force-links `-lkrunfw` with an rpath, so `libkrun.so` resolves
the bundled kernel itself.

On macOS, libkrun needs `com.apple.security.hypervisor` on the running process
(distinct from VZ's `com.apple.security.virtualization`); the one `vmkit` binary
carries both plus `com.apple.security.cs.disable-library-validation`, applied by
the self-signer (see [overview](overview.md)). On Linux no signing is needed
(the process needs read/write access to `/dev/kvm`).

## Capture limitations: GUI stays on VZ

The off-screen Linux **GUI** capture paths (`boot-linux-gui`, `drive-linux`) use
VZ's `VZVirtualMachineView` IOSurface, not libkrun, even though libkrun-efi
exposes a display backend API (`krun_add_display` + `krun_set_display_backend`).
A backend wired to it registers cleanly but does not produce a frame on macOS
with the current libkrun + nixpkgs venus-only `virglrenderer-krunkit` build: the
only path reaching `present_frame` is a virgl GL `glReadPixels` readback, and
this build has no GL backend on macOS (venus/Vulkan only); `SET_SCANOUT_BLOB` is
an unimplemented `panic!`. Capturing via libkrun on macOS needs upstream
Metal-texture scanout work (virglrenderer `create_handle_for_scanout` +
libkrun `SET_SCANOUT_BLOB`), i.e. source patches to two upstream projects. Until
then VZ is the only working Linux-GUI capture path. The headless `boot-linux`
path needs none of this and runs on libkrun on both hosts.

Related constraint: **GPU and Rosetta are mutually exclusive per VM.** Rosetta
for Linux guests is a VZ API (`VZLinuxRosettaDirectoryShare`), so a libkrun VM
can never run x86_64 binaries through Rosetta, and a VZ VM never gets the GPU.
`vmkit`'s VZ Linux paths do not wire Rosetta today.

## Guest images in this domain

- [chrome-vm-image](../chrome-vm-image/overview.md): the aarch64 raw EFI disk
  `boot-linux` boots headless for the [chrome-vm](../chrome-vm/overview.md) demo
  (screenshot back over the console).
- [vz-linux-guest](../vz-linux-guest/overview.md): the aarch64 raw EFI disk
  `boot-linux-gui` boots off-screen under VZ for GUI capture (software graphics
  via lavapipe).
