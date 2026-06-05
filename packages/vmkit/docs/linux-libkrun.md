# Linux guests run on libkrun, not Virtualization.framework

`vmkit` is generic over both the host and the guest OS, with one backend per pair:

- **macOS guests** (macOS host) run on Apple's [Virtualization.framework](https://developer.apple.com/documentation/virtualization) (`src/macguest.rs`, `src/drive.rs`). This is the path that boots an installed macOS, drives it off-screen, and screenshots its framebuffer.
- **Linux guests** run on [libkrun](https://github.com/containers/libkrun) (`src/linuxkrun.rs`), a different libkrun per host:
  - on a **macOS host**, the EFI variant (`libkrun-efi`), which talks to Hypervisor.framework and boots an EFI disk;
  - on a **Linux host**, classic KVM `libkrun`, which boots a rootfs under its bundled kernel.

The macOS split is deliberate. Virtualization.framework gives a Linux guest **no GPU**: a `wgpu`/Vulkan workload falls back to software (Mesa lavapipe). libkrun's macOS variant (libkrun-efi) ships a virtio-gpu **Venus** device backed by [MoltenVK](https://github.com/KhronosGroup/MoltenVK), so a Linux guest gets real Vulkan on the Mac's GPU and a `/dev/dri/renderD128` node. For a Linux VM that needs the GPU on Apple Silicon, libkrun is the only option; this is the same conclusion Podman Desktop, Lima, and colima reached (they all use libkrun/krunkit on macOS).

The two libkrun boot models share the ctx/config/gpu/console/watchdog/`krun_start_enter` skeleton in `src/linuxkrun.rs`; only the payload step is `cfg`-split.

## macOS host: boot an EFI disk under libkrun-efi

`vmkit boot-linux --disk <raw-efi-disk> [--gpu]` boots a raw EFI-bootable disk image (a NixOS `raw-efi` image, a Fedora CoreOS raw, etc.) and streams its serial console until the guest powers off or the timeout elapses. `--gpu` adds the Venus virtio-gpu device.

Call sequence: `krun_create_ctx` → `krun_set_vm_config` → `krun_set_firmware` → `krun_add_disk2` → (optional) `krun_set_gpu_options2` → (optional) `krun_set_console_output` → `krun_start_enter`.

nixpkgs only provides libkrun on Darwin as **libkrun-efi** (classic libkrun's `libkrunfw` kernel package does not build on macOS). The EFI build always boots its embedded OVMF/EDK2 firmware, so `krun_set_kernel` is ignored and a guest must be an EFI-bootable disk image (the same disk shape `VZEFIBootLoader` takes), not a bare kernel + initramfs. `krun_set_firmware` wants a firmware path, so the OVMF blob (`KRUN_EFI.silent.fd`, ~2 MiB, from the libkrun source tree) is embedded into the binary at build time (`KRUN_EFI_FIRMWARE`) and written to a per-user cache file at runtime; embedding keeps the binary self-contained across the entitlement self-sign re-exec.

### What the guest needs for `--gpu`

`--gpu` attaches the device; rendering on it also needs guest-side drivers. The guest kernel must load `virtio_gpu` (that is what creates `/dev/dri/renderD128`), and the guest userspace needs Mesa's [venus](https://docs.mesa3d.org/drivers/venus.html) Vulkan driver. nixpkgs builds venus into `mesa` by default (the `virtio` entry in its `vulkanDrivers`; Mesa 26.1.1 at the current pin), installed as `share/vulkan/icd.d/virtio_icd.aarch64.json`. Select it the way [`vz-linux-guest`](../../vz-linux-guest/nixos.nix) selects lavapipe (`VK_DRIVER_FILES`), or let the loader enumerate it. When the whole stack works, `vulkaninfo --summary` in the guest reports a `Virtio-GPU Venus` device; a lavapipe/`llvmpipe` device means the guest fell back to software and the venus ICD was not loaded.

The known trap is an old guest Mesa: venus against a MoltenVK host needed Mesa fixes that stable-distro Mesas lacked for a while, which is why krunkit's proven guests (Fedora's podman-machine image) shipped a patched Mesa until the fixes landed upstream. A guest with a current Mesa, including a stock NixOS guest built from this repo's pin, has them ([#709](https://github.com/indexable-inc/index/issues/709)).

## Linux host: boot a rootfs under classic libkrun (KVM)

`vmkit boot-linux --root <rootfs-dir> [--gpu] -- <cmd> [args...]` shares a host directory into the guest over virtiofs as `/`, boots it under libkrun's bundled `libkrunfw` kernel, and runs `<cmd>` as the guest init. This is the same model `podman --runtime krun` and `crun` use: no firmware, no guest-supplied kernel, no EFI disk.

Call sequence: `krun_create_ctx` → `krun_set_vm_config` → `krun_set_root` → `krun_set_workdir` → `krun_set_exec` → (optional) `krun_set_gpu_options2` → (optional) `krun_set_console_output` → `krun_start_enter`.

`krun_set_exec(exec_path, argv, envp)` uses `exec_path` as the guest's `argv[0]`, so `argv` is only the *arguments after the binary* (matching libkrun's own `examples/chroot_vm.c`: `krun_set_exec(ctx, guest_argv[0], &guest_argv[1], ...)`). `vmkit` passes `exec[0]` as `exec_path` and `exec[1..]` as `argv`; passing the full vector would duplicate `argv[0]` (e.g. `/bin/sh /bin/sh -c ...`, making the shell try to run its own binary as a script).

A minimal smoke test (verified on x86_64-linux with `/dev/kvm`): a rootfs holding a static busybox, then

```sh
vmkit boot-linux --root ./rootfs --console-file out.log --timeout-secs 30 \
  -- /bin/sh -c 'uname -a; ls -la /; echo VMKIT_BOOT_OK'
```

boots kernel 6.12.x, mounts the rootfs (with `/dev`, `/proc`, `/sys` set up by libkrun), runs the command, and powers off cleanly.

`krun_start_enter` does not return on success on either host: libkrun takes over the process and `exit()`s with the guest's exit code when the VM stops, so the console has streamed by then. A watchdog thread bounds the run so a background invocation never hangs.

## Linking and entitlements

The crate links `-lkrun`, resolved per host by the nix build: `libkrun-efi.dylib` on macOS (`${libkrun-efi}/lib`), classic `libkrun.so` on Linux (`${libkrun}/lib64`). The build script emits the `-l` (gated on `have_libkrun`, set when libkrun is available for the build host); the search path and rpath are injected by the workspace build (`lib/rust/workspace.nix`), because a build script's link-search does not reach the final unit link in the cargo-unit graph. On Linux, nixpkgs `libkrun` force-links `-lkrunfw` with an rpath, so `libkrun.so` resolves the bundled kernel itself at runtime: only libkrun's own lib dir must reach the `vmkit` binary's rpath.

On **macOS**, libkrun needs `com.apple.security.hypervisor` on the running process (distinct from Virtualization.framework's `com.apple.security.virtualization`). The one `vmkit` binary carries both, plus `com.apple.security.cs.disable-library-validation` so it can load the ad-hoc-signed libkrun dylib from the Nix store. The self-signer (`src/main.rs`) applies these on first run. On **Linux** no signing is needed: classic libkrun talks to `/dev/kvm` directly (the process needs read/write access to `/dev/kvm`).

## Known limitations

- **GUI off-screen capture stays on Virtualization.framework; libkrun cannot do it on macOS yet.** The GUI paths (`boot-linux-gui`, `drive-linux`) use VZ's `VZVirtualMachineView` IOSurface. libkrun-efi *exposes* a display backend API (`krun_add_display` + `krun_set_display_backend` with `configure_scanout`/`alloc_frame`/`present_frame`, plus `krun_add_input_device`), and a backend wired to it registers cleanly, but it does **not** produce a frame on macOS with the current libkrun + nixpkgs `virglrenderer-krunkit` (venus-only) build. The only path that reaches `present_frame` is `flush_resource` → `virgl_renderer_transfer_read_iov`, a virgl **GL** readback (`glReadPixels`) for every resource type, and this build has no GL backend on macOS (venus/Vulkan only). Venus rendering does not bypass it: `SET_SCANOUT_BLOB` is an unimplemented `panic!`, `set_scanout` rejects blob resources (it requires a 2D format), and the readback is still GL. The `RENDER_SERVER` flag does not apply (libkrun never wires a render-server fd, and it is not a new backend). Capturing a guest framebuffer via libkrun on macOS needs the upstream Metal-texture scanout work (a new unstable `virgl_renderer_create_handle_for_scanout` in virglrenderer plus `SET_SCANOUT_BLOB` support in libkrun, tracked by the UTM venus-on-macOS effort, Dec 2025), i.e. source patches to two upstream projects, not a flag or nixpkgs overlay. Until that lands, VZ is the only working Linux-GUI capture path. (The headless `boot-linux` path needs none of this and runs on libkrun on both hosts.)
- **GPU and Rosetta are mutually exclusive per VM.** Rosetta translation for Linux guests is a Virtualization.framework API ([`VZLinuxRosettaDirectoryShare`](https://developer.apple.com/documentation/virtualization/vzlinuxrosettadirectoryshare)), so a libkrun VM can never run x86_64 binaries through Rosetta, and a VZ VM never gets the GPU. Pick the backend by workload. Today the choice is theoretical on the Rosetta side: no `vmkit` path wires Rosetta into its VZ Linux guests (`boot-linux-gui`, `drive-linux`), so adding it is work on the VZ paths.
- **macOS-guest paths are aarch64-darwin only.** libkrun-efi is packaged only for Apple Silicon, and the macOS-guest boot path is exercised only there. The Linux-host backend covers `aarch64-linux` and `x86_64-linux`.
- **The guest shape differs by host.** On macOS a guest is an EFI disk (its own kernel/bootloader); on Linux a guest is a rootfs directory run under libkrun's bundled kernel. The `--disk` flag exists only on macOS, `--root`/`-- <cmd>` only on Linux.
