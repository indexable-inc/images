# Linux guests run on libkrun, not Virtualization.framework

`macos-vm` is generic over the guest OS, with one backend per guest:

- **macOS guests** run on Apple's [Virtualization.framework](https://developer.apple.com/documentation/virtualization) (`src/macguest.rs`, `src/drive.rs`). This is the path that boots an installed macOS, drives it off-screen, and screenshots its framebuffer.
- **Linux guests** run on [libkrun](https://github.com/containers/libkrun) (`src/linuxkrun.rs`), which talks to Hypervisor.framework directly.

The split is deliberate. Virtualization.framework gives a Linux guest **no GPU**: a `wgpu`/Vulkan workload falls back to software (Mesa lavapipe). libkrun's macOS variant (libkrun-efi) ships a virtio-gpu **Venus** device backed by [MoltenVK](https://github.com/KhronosGroup/MoltenVK), so a Linux guest gets real Vulkan on the Mac's GPU and a `/dev/dri/renderD128` node. For a Linux VM that needs the GPU on Apple Silicon, libkrun is the only option; this is the same conclusion Podman Desktop, Lima, and colima reached (they all use libkrun/krunkit on macOS).

## What the Linux backend does

`macos-vm boot-linux --disk <raw-efi-disk> [--gpu]` boots a raw EFI-bootable disk image (a NixOS `raw-efi` image, a Fedora CoreOS raw, etc.) and streams its serial console until the guest powers off or the timeout elapses. `--gpu` adds the Venus virtio-gpu device.

The call sequence (`src/linuxkrun.rs`): `krun_create_ctx` → `krun_set_vm_config` → `krun_set_firmware` → `krun_add_disk2` → (optional) `krun_set_gpu_options2` → (optional) `krun_set_console_output` → `krun_start_enter`. `krun_start_enter` does not return on success: libkrun takes over the process and `exit()`s with the guest's exit code when the VM stops, so the console has streamed by then.

## Why the EFI variant, and the firmware

nixpkgs only provides libkrun on Darwin as **libkrun-efi** (classic libkrun's `libkrunfw` kernel package does not build on macOS). The EFI build always boots its embedded OVMF/EDK2 firmware, so `krun_set_kernel` is ignored and a guest must be an EFI-bootable disk image (the same disk shape `VZEFIBootLoader` takes), not a bare kernel + initramfs.

`krun_set_firmware` wants a firmware path. The OVMF blob (`KRUN_EFI.silent.fd`, ~2 MiB) lives in the libkrun source tree and is embedded into the binary at build time (`KRUN_EFI_FIRMWARE`, set by the nix build to `${libkrun-efi.src}/edk2/KRUN_EFI.silent.fd`). Embedding keeps the binary self-contained across the entitlement self-sign re-exec; at runtime the bytes are written to a per-user cache file whose path is handed to `krun_set_firmware`.

## Linking and entitlements

The crate links `-lkrun` (which resolves to `libkrun-efi.dylib` through the nix package's symlink chain). The build script emits the `-l`; the search path and rpath are injected by the workspace build (`lib/rust/workspace.nix`), because a build script's link-search does not reach the final unit link in the cargo-unit graph.

libkrun needs `com.apple.security.hypervisor` on the running process (distinct from Virtualization.framework's `com.apple.security.virtualization`). The one `macos-vm` binary carries both, plus `com.apple.security.cs.disable-library-validation` so it can load the ad-hoc-signed libkrun dylib from the Nix store. The self-signer (`src/main.rs`) applies these on first run.

## Known limitations

- **GUI capture stays on Virtualization.framework.** The off-screen framebuffer capture and synthetic-input paths (`boot-linux-gui`, `drive-linux`) still use VZ's `VZVirtualMachineView` IOSurface, which libkrun has no direct equivalent for yet. Migrating them to libkrun's virtio-gpu is follow-up work; until then those two commands keep their VZ implementation.
- **aarch64-darwin only.** libkrun-efi is packaged only for Apple Silicon.
- **A guest is an EFI disk, not a kernel + initramfs.** The previous VZ `boot-linux` accepted a raw kernel + initramfs; libkrun-efi boots an EFI disk instead.
