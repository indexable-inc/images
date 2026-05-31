# OCI image as a guest: build vs. delegate

Roadmap item from [issue #453](https://github.com/indexable-inc/index/issues/453):
run an OCI image as a `macos-vm` guest, reachable by the existing capture and
drive tooling. This note records the decision (build vs. delegate) and the
minimal proof that backs it.

## Decision

Keep `macos-vm` focused on the macOS-guest plus capture/drive niche on
[Virtualization.framework](https://developer.apple.com/documentation/virtualization),
and delegate the general Linux-container case to a purpose-built microVM runtime
([libkrun](https://github.com/containers/libkrun) / krunkit). `macos-vm` owns one
small, honest OCI path: boot a flattened OCI rootfs as a virtio-blk disk through
the existing Linux boot path, which is enough to drive a containerized Linux
workload through the same serial console the rest of the crate already uses. It
does not try to be a container runtime.

The reasoning, grounded in what each backend can and cannot do:

- **No 3D acceleration for Linux on Virtualization.framework.** VZ accelerates
  the macOS guest (`VZMacGraphicsDeviceConfiguration`) but gives a Linux guest no
  GPU: `wgpu` in a VZ Linux guest falls back to software (lavapipe). libkrun's
  `libkrun-efi` adds a Venus virtio-gpu backed by
  [MoltenVK](https://github.com/KhronosGroup/MoltenVK), so a Linux guest gets
  real Vulkan. Any OCI workload that needs the GPU is libkrun's job, not ours;
  rebuilding that path on VZ is not justified.
- **Different entitlements, owned by different processes.** VZ needs
  `com.apple.security.virtualization` (see `src/virtualization.entitlements` and
  the self-signer in `src/main.rs`). libkrun uses Hypervisor.framework and needs
  `com.apple.security.hypervisor`. They are separate code-signing surfaces;
  delegating the container case keeps this crate's single entitlement and its
  self-signing story unchanged.
- **libkrun already owns the container surface.** It boots an OCI image as a
  microVM in milliseconds with the image as the rootfs over virtio-fs. Matching
  that (image pull, layer cache, fast boot, GPU) inside `macos-vm` is a large,
  duplicative effort. The deliberate single-backend decision, if it is ever
  forced, should be made on its own, not reached by accretion here.

What `macos-vm` does own, because it costs almost nothing on top of the existing
`boot-linux` path: attach an OCI-derived rootfs disk and reach its userspace.
That is the proof below.

## How an OCI image becomes a bootable disk

The repo already turns an OCI image into a flat rootfs. `packages/oci-image-builder`
(the `oci-image-builder` binary) and `lib/ix-oci-layer.nix` build OCI archives
from a NixOS closure: `ix-oci-layer.nix` streams the closure into ~67 OCI layers
with `dockerTools.streamLayeredImage` and hands the layer plan to
`oci-image-builder`, which writes the final OCI archive. Flattening the layers of
any OCI image (those layers, or an external image such as `busybox`) into a
single directory tree gives a rootfs; that tree becomes a disk image with
`mke2fs -d` (ext*) or `mksquashfs` (squashfs), neither of which needs root or a
loopback mount.

## Why this needed a new boot path

Before this change, `boot-linux` accepted only a kernel `Image` and an
initramfs. It built no storage device, so there was no way to hand a guest the
flattened rootfs disk. Two ways to close that gap:

1. **Kernel + initrd that mounts a rootfs disk** (the path taken). Attach the
   rootfs as a virtio-blk disk and let a small initramfs `switch_root` into it,
   or point the kernel at it with `root=/dev/vda`. This reuses
   `VZLinuxBootLoader` unchanged; it only needs a virtio-blk device on the
   config.
2. **A full EFI/disk boot path.** `VZEFIBootLoader` plus
   `VZDiskImageStorageDeviceAttachment` boots a disk that carries its own
   bootloader and kernel (an EFI system partition), with no host-supplied kernel
   or initramfs. This is the right shape for a self-contained bootable image, but
   it is a larger surface (EFI variable store, a partitioned disk) than this
   lower-priority item warrants.

This change took option 1: a minimal `--disk` flag on `boot-linux` that attaches
one or more raw disk images as virtio-blk devices
(`VZVirtioBlockDeviceConfiguration` + `VZDiskImageStorageDeviceAttachment`, the
same pair `macguest.rs` already uses for the macOS disk). The guest sees them as
`/dev/vda`, `/dev/vdb`, ... in order. An EFI boot path remains available later if
a self-contained bootable OCI disk is wanted.

## The proof

`examples/oci-boot.sh` runs the whole path from a clean temp dir:

1. Pull `busybox:latest` (arm64) from Docker Hub with `skopeo` and flatten its
   one layer into a rootfs tree.
2. Drop in a tiny `/init` that prints a marker from inside the OCI userspace.
3. Pack the tree into a read-only squashfs disk with `mksquashfs`.
4. Fetch an Alpine aarch64 `virt` kernel, extract the raw arm64 `Image` from its
   gzip zboot `vmlinuz` wrapper, and build a small initramfs that loads
   `virtio_blk` and `squashfs`, mounts `/dev/vda`, and `switch_root`s into it.
5. Boot with `macos-vm boot-linux --disk rootfs.squashfs`, streaming the serial
   console.

Observed serial console (the load-bearing lines):

```
INITRAMFS: loading virtio_blk + squashfs
[    0.157397] virtio_blk virtio1: [vda] 3760 512-byte logical blocks (1.93 MB/1.84 MiB)
[    0.158672] squashfs: version 4.0 (2009/01/31) Phillip Lougher
INITRAMFS: mounted OCI squashfs; switch_root
OCI-GUEST-PROOF: reached userspace from OCI rootfs on /dev/vda
uname: Linux (none) 6.12.81-0-virt #1-Alpine SMP PREEMPT_DYNAMIC ... aarch64 GNU/Linux
root contents: bin dev etc home init lib lib64 root tmp usr var
```

The `root contents` are the busybox OCI image's directories, so the guest is
running from the OCI-derived disk attached over the new `--disk` flag, reached
through the same serial-console path the existing `boot-linux` smoke test uses.

## Limits and remaining gaps

- **squashfs, not ext4, in this proof.** The Alpine `virt` kernel builds no
  ext2/3/4 in (its `/proc/filesystems` lists only `nodev` filesystems plus the
  loadable squashfs and vfat modules), so the proof uses a read-only squashfs
  disk and loads `squashfs.ko` from the Alpine initramfs. squashfs is a good fit
  for an immutable OCI image. A writable ext4 root would need either a kernel
  with ext4 built in or the ext4 module loaded the same way virtio_blk is.
- **External downloads, and no nix-built aarch64 kernel.** The example fetches
  the kernel from Alpine and the image from Docker Hub. There is no
  aarch64-linux kernel built through nix here because this host's only remote
  builder is x86_64-linux; producing a raw arm64 `Image` reproducibly through
  nix is follow-up work. The raw-`Image` requirement is real: `VZLinuxBootLoader`
  takes an uncompressed arm64 `Image`, not a gzip/zboot `vmlinuz`, which is why
  the script extracts the inner `Image`.
- **Capture/drive is serial-console only for Linux.** The framebuffer capture
  and synthetic-input tooling (`boot-macos` / `drive-macos`) target the macOS
  guest's `VZVirtualMachineView`. A Linux OCI guest is reachable here through the
  serial console; wiring a Linux guest into the framebuffer/input surface (and
  the GPU question above) is exactly the case that belongs to libkrun.
- **No image pull, cache, or `root=` ergonomics in the binary.** The pull,
  flatten, and disk build live in the example script, not in `macos-vm`. The
  binary's contribution is the `--disk` attachment; turning an OCI reference into
  a booted guest in one command is not built, by design (that is the runtime
  surface we are delegating).
