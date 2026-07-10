# metal-httpd

One tiny HTTP server, four ways down the stack. The same `no_std` request
handler ([`http-core`](http-core/src/lib.rs)) is served from a normal Linux
process all the way down to a freestanding kernel that owns the NIC — the
point is that "porting to bare metal" can be a *backend selection* rather
than a rewrite, and each rung of the ladder swaps out one more layer of the
OS:

| backend  | what runs under the app                          | libc            | network stack            | boots as                  |
| -------- | ------------------------------------------------ | --------------- | ------------------------ | ------------------------- |
| `linux`  | full OS (std → glibc → kernel)                   | glibc           | Linux                    | host process              |
| `eyra`   | full OS, but libc is pure Rust ([Eyra])          | [c-ward] (Rust) | Linux                    | host process (no C at all)|
| `hermit` | [Hermit] unikernel linked into the app image     | none (std→ABI)  | smoltcp (in-kernel)      | QEMU `-kernel` image      |
| `bare`   | nothing — [`kernel/`](kernel/src/main.rs) *is* the OS | none       | smoltcp (in [`kernel/`](kernel/src/net.rs)) | QEMU BIOS disk image |

[Eyra]: https://github.com/sunfishcode/eyra
[c-ward]: https://github.com/sunfishcode/c-ward
[Hermit]: https://github.com/hermit-os/kernel

Every backend answers `GET /` with a body that names it, so the e2e harness
can prove which image actually served the request:

```text
hello from metal-httpd
backend: bare
path: /
```

## Quick start

```sh
cd packages/metal-httpd
cargo xtask e2e            # build + boot + curl all four backends
cargo xtask e2e bare       # just one of them
cargo xtask build hermit   # build artifacts without running
```

`e2e` launches each backend (host process for `linux`/`eyra`, QEMU for
`hermit`/`bare`), sends a real HTTP request from the host, and asserts on
the `backend:` line in the response. Requirements: the repo's pinned nightly
toolchain (rustup handles that), `qemu-system-x86_64` on `PATH` for the two
image backends, and network access the first time (crates, hermit sources).

## How each backend is selected

The modularity lives in ordinary Rust mechanisms — no forked source trees:

- **`linux`** — `cargo build -p httpd`. Plain std.
- **`eyra`** — `cargo build -p httpd --features eyra`. The feature pulls in
  the [`eyra`](https://github.com/sunfishcode/eyra) crate, whose `c-scape`/
  `c-gull` crates provide libc's ABI in pure Rust; `build.rs` adds
  `-nostartfiles` so even crt0 comes from Rust. Same source, and `ldd`
  reports `statically linked` — no C runtime in the binary.
- **`hermit`** — `cargo build -p httpd --target x86_64-unknown-hermit
  -Zbuild-std=std,panic_abort`. std's OS layer retargets to the Hermit
  unikernel; the `hermit` dependency (enabled only for that target) links
  the kernel — with its virtio-net driver and smoltcp stack — into the app
  image. Booted via the [hermit-loader], which xtask fetches and builds
  (override with `HERMIT_LOADER=/path/to/loader`).
- **`bare`** — `cargo build -p kernel --target x86_64-unknown-none`, then
  xtask wraps the ELF into a BIOS-bootable disk image with the
  [`bootloader`](https://crates.io/crates/bootloader) crate. The kernel
  brings its own serial console, physical-memory/heap management, PCI
  enumeration (port-IO CAM), virtio-net driver glue ([virtio-drivers]) and
  TCP/IP ([smoltcp]), then runs the http-core accept loop fully polled — no
  interrupts, no threads, no syscalls.

[hermit-loader]: https://github.com/hermit-os/loader
[virtio-drivers]: https://crates.io/crates/virtio-drivers
[smoltcp]: https://crates.io/crates/smoltcp

`httpd/` (std backends) and `kernel/` (freestanding backend) are separate
binaries because a freestanding target genuinely has no std to share — but
both are thin transport shims (~150 lines) around the same `http-core`
request parser and response writer, which is unit-tested once on the host.

## Networking

All virtualized backends assume QEMU user-mode networking with a virtio-net
NIC (`-device virtio-net-pci`), which is what xtask sets up:

- guest address `10.0.2.15/24`, gateway `10.0.2.2` (QEMU slirp defaults) —
  hermit gets it via DHCP, the bare kernel uses it statically;
- the server's port 8080 is reached through `hostfwd` on an ephemeral host
  loopback port;
- serial output lands in `target/e2e/<backend>/serial.log`, and the e2e
  harness prints the tail on failure.

## Caveats, honestly

- The whole workspace expects the repo's pinned nightly (`build-std` for
  hermit, Eyra's `-nostartfiles` linkage, `x86_64-unknown-none`).
- hermit is pinned to 0.12 with a curated feature list; notably
  `virtio-net` must stay on (without it the kernel silently falls back to a
  loopback interface and the server is unreachable) and `smp` stays off
  (the 0.12 kernel trips over the loader's huge-page mappings when booting
  secondary cores under QEMU).
- The bare kernel is a demo OS: single CPU, polled I/O, TSC timekeeping
  assuming 1 GHz, fixed TCP seed, and it trusts the bootloader's huge-page
  physical-memory mapping to reach MMIO BARs (fine under QEMU/TCG, where
  page-attribute caching semantics are not emulated).
- `[patch.crates-io]` pins Eyra's `c-scape`/`c-gull` to an upstream commit
  because the released 0.22.3 no longer compiles on current nightlies
  (printf-compat 0.3's `VaList` usage); drop it when c-ward releases.
