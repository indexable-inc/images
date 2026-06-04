#!/usr/bin/env bash
# Boot an OCI image as a macos-vm guest disk, end to end.
#
# This is the minimal honest proof for the OCI-as-guest roadmap item (issue
# #453, see ../docs/oci-guest.md). It flattens a tiny OCI image into a read-only
# squashfs disk, attaches that disk to a Linux guest with `macos-vm boot-linux
# --disk`, and switches root into the OCI rootfs from a small initramfs, so the
# serial console prints a marker from inside the OCI image's userspace.
#
# It downloads two things from the network and says so up front:
#   - the busybox arm64 OCI image from Docker Hub (the guest rootfs), and
#   - an Alpine aarch64 `virt` kernel + its modules (the kernel + virtio_blk and
#     squashfs drivers). macos-vm's `boot-linux` takes a raw arm64 `Image`; the
#     Alpine `vmlinuz` is a gzip zboot wrapper, so we extract the inner `Image`.
# Everything else (mke2fs/mksquashfs/skopeo) comes from nixpkgs via `nix run`.
#
# Requirements: aarch64-darwin with Virtualization.framework, network access.
#
# Usage:
#   packages/macos-vm/examples/oci-boot.sh [OCI_REF]
# OCI_REF defaults to docker.io/library/busybox:latest.
set -euo pipefail

OCI_REF="${1:-docker.io/library/busybox:latest}"
ALPINE_NETBOOT="${ALPINE_NETBOOT:-https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/aarch64/netboot}"
WORK="$(mktemp -d -t oci-boot.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT
echo "macos-vm oci-boot: workdir $WORK"

# Resolve macos-vm: prefer a built result, else build it.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
MACVM="$ROOT/result/bin/macos-vm"
if [[ ! -x "$MACVM" ]]; then
  echo "macos-vm oci-boot: building .#macos-vm"
  nix build "$ROOT#macos-vm" -o "$WORK/macos-vm-result"
  MACVM="$WORK/macos-vm-result/bin/macos-vm"
fi

# Run a named binary from a nixpkgs package (the package and the binary differ
# for multi-tool packages like squashfsTools, so name both).
nixbin() { nix shell "nixpkgs#$1" -c "${@:2}"; }

# 1. Pull the OCI image (arm64) and flatten its layers into a rootfs tree.
echo "macos-vm oci-boot: pulling OCI image $OCI_REF (arm64)"
mkdir -p "$WORK/oci" "$WORK/root"
echo '{ "default": [ { "type": "insecureAcceptAnything" } ] }' >"$WORK/policy.json"
nixbin skopeo skopeo --policy "$WORK/policy.json" --override-os linux --override-arch arm64 \
  copy "docker://$OCI_REF" "oci:$WORK/oci:img"
python3 - "$WORK" <<'PY'
import json, sys, subprocess, pathlib
work = pathlib.Path(sys.argv[1])
idx = json.load(open(work / "oci/index.json"))
man_dig = idx["manifests"][0]["digest"].split(":")[1]
man = json.load(open(work / "oci/blobs/sha256" / man_dig))
for layer in man["layers"]:
    blob = work / "oci/blobs/sha256" / layer["digest"].split(":")[1]
    # OCI layers are (usually gzipped) tarballs; tar autodetects compression.
    subprocess.run(["tar", "-xpf", str(blob), "-C", str(work / "root")], check=True)
print("flattened", len(man["layers"]), "layer(s)")
PY

# 2. A tiny in-rootfs /init that proves we reached the OCI image's userspace.
cat >"$WORK/root/init" <<'EOF'
#!/bin/sh
/bin/busybox mount -t proc proc /proc 2>/dev/null
/bin/busybox mount -t sysfs sys /sys 2>/dev/null
echo "=================================================="
echo "OCI-GUEST-PROOF: reached userspace from OCI rootfs on /dev/vda"
echo "uname: $(/bin/busybox uname -a)"
echo "root contents: $(/bin/busybox ls / | /bin/busybox tr '\n' ' ')"
echo "OCI-GUEST-PROOF-DONE"
echo "=================================================="
/bin/busybox poweroff -f
EOF
chmod +x "$WORK/root/init"

# 3. Flatten the rootfs into a read-only squashfs disk. squashfs fits an
#    immutable OCI image and is one of the few block filesystems the Alpine
#    virt kernel can mount (it has no ext4 built in).
echo "macos-vm oci-boot: building squashfs disk"
nixbin squashfsTools mksquashfs "$WORK/root" "$WORK/rootfs.squashfs" \
  -noappend -all-root -comp gzip

# 4. Fetch the Alpine kernel + initramfs and extract a raw arm64 Image.
echo "macos-vm oci-boot: fetching Alpine aarch64 kernel + modules"
curl -fsSL -o "$WORK/vmlinuz-virt" "$ALPINE_NETBOOT/vmlinuz-virt"
curl -fsSL -o "$WORK/initramfs-virt" "$ALPINE_NETBOOT/initramfs-virt"
python3 - "$WORK" <<'PY'
import zlib, sys, pathlib
work = pathlib.Path(sys.argv[1])
data = (work / "vmlinuz-virt").read_bytes()
# The arm64 zboot wrapper embeds a gzip stream with trailing payload after it.
# zlib's streaming gzip mode (wbits 16+15) decompresses the leading stream and
# ignores the trailing bytes, where the strict gzip.decompress rejects them.
off = 0
while True:
    i = data.find(b"\x1f\x8b\x08", off)
    if i < 0:
        sys.exit("no raw arm64 Image found inside vmlinuz zboot wrapper")
    try:
        raw = zlib.decompressobj(16 + zlib.MAX_WBITS).decompress(data[i:])
    except Exception:
        off = i + 3
        continue
    # arm64 raw Image header magic "ARM\x64" at offset 56 (Documentation/arm64/booting).
    if len(raw) > 1_000_000 and raw[56:60] == b"ARM\x64":
        (work / "Image").write_bytes(raw)
        print("extracted raw arm64 Image:", len(raw), "bytes")
        break
    off = i + 3
PY

# 5. Build a minimal initramfs that insmods virtio_blk + squashfs (the Alpine
#    virt kernel ships them as modules), mounts the OCI squashfs disk, and
#    switch_roots into it. The busybox here is the OCI image's own (glibc), so
#    copy its loader + libs in too.
echo "macos-vm oci-boot: building initramfs"
IR="$WORK/irfs"
mkdir -p "$IR"/{bin,lib,mods,proc,sys,dev,mnt}
cp "$WORK/root/bin/busybox" "$IR/bin/busybox"
for lib in ld-linux-aarch64.so.1 libc.so.6 libm.so.6 libresolv.so.2; do
  [[ -e "$WORK/root/lib/$lib" ]] && cp "$WORK/root/lib/$lib" "$IR/lib/"
done
# Extract the two kernel modules from the Alpine initramfs. Use `gzip -dc`, not
# `zcat`: macOS ships BSD `zcat`, which appends `.Z` and decodes compress(1), so
# `zcat initramfs-virt` looks for `initramfs-virt.Z` and fails on a gzip file.
( cd "$WORK" && mkdir -p aird && cd aird && gzip -dc ../initramfs-virt | cpio -idm 2>/dev/null )
# The kernel-version directory name is unknown, so glob it (one match expected).
kmods=( "$WORK"/aird/lib/modules/*/kernel )
KMOD="${kmods[0]}"
cp "$KMOD/drivers/block/virtio_blk.ko" "$IR/mods/"
cp "$KMOD/fs/squashfs/squashfs.ko" "$IR/mods/"
cat >"$IR/init" <<'EOF'
#!/bin/busybox sh
/bin/busybox mount -t devtmpfs dev /dev 2>/dev/null
/bin/busybox mount -t proc proc /proc 2>/dev/null
echo "INITRAMFS: loading virtio_blk + squashfs"
/bin/busybox insmod /mods/virtio_blk.ko
/bin/busybox insmod /mods/squashfs.ko
for i in 1 2 3 4 5 6 7 8 9 10; do [ -b /dev/vda ] && break; /bin/busybox sleep 0.3; done
/bin/busybox mount -t squashfs -o ro /dev/vda /mnt || { echo "INITRAMFS: mount FAILED"; /bin/busybox poweroff -f; }
echo "INITRAMFS: mounted OCI squashfs; switch_root"
exec /bin/busybox switch_root /mnt /init
EOF
chmod +x "$IR/init"
( cd "$IR" && find . | cpio -o -H newc 2>/dev/null | gzip -9 >"$WORK/initramfs.cpio.gz" )

# 6. Boot it. The OCI disk is attached via the new --disk flag (virtio-blk).
echo "macos-vm oci-boot: booting guest"
"$MACVM" boot-linux \
  --kernel "$WORK/Image" \
  --initramfs "$WORK/initramfs.cpio.gz" \
  --disk "$WORK/rootfs.squashfs" \
  --memory-mib 1024 \
  --cmdline "console=hvc0" \
  --timeout-secs 45
