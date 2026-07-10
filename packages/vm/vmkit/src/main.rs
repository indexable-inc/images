//! `vmkit`: own a guest VM's lifecycle from Rust, one hypervisor backend per host
//! and guest OS.
//!
//! - **macOS guests** run on Apple's Virtualization.framework ([`macguest`],
//!   [`drive`], and [`linuxguest`] for the off-screen GUI-capture paths).
//! - **Linux guests** run on **libkrun** ([`linuxkrun`]): the EFI / Hypervisor.framework
//!   variant on a macOS host (the only backend that gives a Linux guest a GPU on
//!   Apple Silicon), and classic KVM libkrun on a Linux host (the bundled kernel
//!   boots a rootfs + exec command, the same model `podman --runtime krun` uses).
//!
//! On macOS the binary owns the VM so callers that cannot hold the entitlements
//! themselves (notably the ix-mcp Python interpreter, an unsigned immutable Nix
//! store binary) can spawn it and control a VM over IPC. The
//! `com.apple.security.virtualization` (VZ) and `com.apple.security.hypervisor`
//! (libkrun) entitlements live on *this* signed process, never on the interpreter
//! (see [`ensure_signed_and_reexec`]). On a Linux host no signing is needed:
//! classic libkrun talks to `/dev/kvm` directly.
//!
//! `vmkit info` reports whether virtualization is available. `vmkit boot-linux`
//! boots a Linux guest under libkrun and streams its console; the guest argument
//! differs by host (a raw EFI disk `--disk` on macOS, a rootfs directory `--root`
//! plus a trailing exec command on Linux). The macOS-guest, GUI-capture, and
//! provisioning paths are macOS-only and tracked in the README and
//! `docs/linux-libkrun.md`.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "vmkit",
    about = "Own a guest VM's lifecycle: macOS guests on Virtualization.framework, Linux guests on libkrun"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Report whether this host can run a VM (Virtualization.framework on macOS,
    /// `/dev/kvm` + libkrun on Linux).
    Info,
    /// Boot a Linux guest under libkrun, streaming its serial console until the
    /// guest powers off or the timeout elapses.
    ///
    /// On macOS the guest is a raw EFI-bootable disk (`--disk`) booted by
    /// libkrun-efi's embedded OVMF; libkrun is the only backend that can give a
    /// Linux guest GPU acceleration on Apple Silicon (`--gpu`). On Linux the guest
    /// is a rootfs directory (`--root`) booted by libkrun's bundled kernel,
    /// running the trailing command as the guest init.
    BootLinux {
        /// macOS host: path to a raw disk image, repeatable. The first `--disk`
        /// is the EFI boot disk (a NixOS `raw-efi` image or a Fedora CoreOS
        /// raw) whose own kernel/bootloader libkrun's embedded OVMF firmware
        /// boots; each further `--disk` attaches in order as an extra
        /// virtio-blk device the guest sees as /dev/vdb, /dev/vdc, ... (e.g. a
        /// persistent data disk that survives boot-image swaps).
        #[cfg(target_os = "macos")]
        #[arg(long = "disk", value_name = "DISK", required = true)]
        disks: Vec<std::path::PathBuf>,
        /// Linux host: path to a rootfs directory, shared into the guest over
        /// virtiofs as `/` and booted by libkrun's bundled kernel.
        #[cfg(target_os = "linux")]
        #[arg(long)]
        root: std::path::PathBuf,
        /// Linux host: the command run as the guest init (argv). `exec[0]` is an
        /// absolute path inside the rootfs; defaults to `/bin/sh`. Pass after
        /// `--`, e.g. `vmkit boot-linux --root ./rootfs -- /bin/ls -la /`.
        #[cfg(target_os = "linux")]
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        exec: Vec<String>,
        /// Enable a virtio-gpu Venus device so the guest gets a real GPU
        /// (`/dev/dri`). On macOS this is the only way a Linux guest gets a GPU
        /// (Vulkan via `MoltenVK`). Off by default.
        #[arg(long)]
        gpu: bool,
        /// Number of virtual CPUs. Typed to libkrun's `u8` so an out-of-range
        /// value is rejected at parse time rather than silently clamped (libkrun
        /// further caps the usable count and reports its own error).
        #[arg(long, default_value_t = 2)]
        cpus: u8,
        /// Guest memory in MiB. Typed to libkrun's `u32` so an out-of-range value
        /// is rejected at parse time rather than silently clamped.
        #[arg(long, default_value_t = 1024)]
        memory_mib: u32,
        /// Forward a host TCP port to the guest, `HOST:GUEST`, repeatable (e.g.
        /// `--port 3200:3200`). Implies `--net`; the guest service is reached on
        /// the host at HOST. On macOS forwarding is done by gvproxy, on Linux by
        /// libkrun's TSI port map.
        #[arg(long = "port", value_name = "HOST:GUEST")]
        ports: Vec<String>,
        /// Give the guest outbound network even with no `--port` forwards. A guest
        /// NIC (gvproxy on macOS, TSI on Linux) that NATs to the host network.
        #[arg(long)]
        net: bool,
        /// Expose a guest `AF_VSOCK` port as a host unix socket,
        /// `GUEST_PORT:HOST_PATH`, repeatable (e.g. `--vsock-port
        /// 7100:/tmp/guest.sock`). libkrun listens on `HOST_PATH` and forwards
        /// each host `connect()` into the guest's vsock `GUEST_PORT`, so a guest
        /// service that listens on vsock is reached from the host by connecting
        /// to the unix socket. Independent of `--net`/`--port` (vsock is its own
        /// device, not the NIC).
        #[arg(long = "vsock-port", value_name = "GUEST_PORT:HOST_PATH")]
        vsock_ports: Vec<String>,
        /// Capture the guest serial console to this file instead of the process's
        /// stdout (useful for a background/lockstep caller).
        #[arg(long)]
        console_file: Option<std::path::PathBuf>,
        /// Stop the VM and exit after this many seconds; `0` runs until the guest
        /// powers off (the persistent-server case).
        #[arg(long, default_value_t = 20)]
        timeout_secs: u64,
    },
    /// Boot an aarch64 Linux GUI guest from a raw EFI disk with a virtio-gpu
    /// display + USB keyboard/mouse, fully off-screen, and screenshot its
    /// framebuffer to `<out-prefix>.NNN.png` (no window, host cursor untouched).
    #[cfg(target_os = "macos")]
    BootLinuxGui {
        /// Path to a raw EFI-bootable disk image (e.g. a NixOS `raw-efi` image).
        #[arg(long)]
        disk: std::path::PathBuf,
        /// EFI variable store file (created if absent). Defaults to
        /// `<disk>.efivars`.
        #[arg(long)]
        efi_vars: Option<std::path::PathBuf>,
        /// Output path prefix for screenshots.
        #[arg(long)]
        out_prefix: std::path::PathBuf,
        /// Stop the VM and exit after this many seconds (final shot at the
        /// deadline).
        #[arg(long, default_value_t = 40)]
        seconds: u64,
        /// Number of virtual CPUs.
        #[arg(long, default_value_t = 4)]
        cpus: usize,
        /// Guest memory in MiB.
        #[arg(long, default_value_t = 4096)]
        memory_mib: u64,
    },
    /// Boot an aarch64 Linux GUI guest from a raw EFI disk off-screen and drive it
    /// from stdin: synthetic keyboard/mouse and on-demand framebuffer screenshots,
    /// with no host cursor or visible window. Same newline command protocol as
    /// `drive-macos` (`key`/`down`/`up`/`type`/`click`/`wait`/`shot`/`quit`).
    #[cfg(target_os = "macos")]
    DriveLinux {
        /// Path to a raw EFI-bootable disk image (e.g. a NixOS `raw-efi` image).
        #[arg(long)]
        disk: std::path::PathBuf,
        /// EFI variable store file (created if absent). Defaults to
        /// `<disk>.efivars`.
        #[arg(long)]
        efi_vars: Option<std::path::PathBuf>,
        /// Number of virtual CPUs.
        #[arg(long, default_value_t = 4)]
        cpus: usize,
        /// Guest memory in MiB.
        #[arg(long, default_value_t = 4096)]
        memory_mib: u64,
    },
    /// Install macOS into a fresh bundle directory from a local restore image
    /// (IPSW). Bypasses Apple's online catalog (take a downloaded `.ipsw`).
    #[cfg(target_os = "macos")]
    InstallMacos {
        /// Path to a macOS restore image (`UniversalMac_*.ipsw`).
        #[arg(long)]
        ipsw: std::path::PathBuf,
        /// Bundle directory to create (disk, aux, hardware-model, machine-id).
        #[arg(long)]
        bundle: std::path::PathBuf,
        /// Main disk size in GiB.
        #[arg(long, default_value_t = 64)]
        disk_gib: u64,
    },
    /// Boot an installed macOS guest fully off-screen and screenshot its display
    /// to `<out-prefix>.NNN.png` via the framebuffer `IOSurface` (no window, no
    /// Screen-Recording permission). The bundle is a directory with `disk.img`,
    /// `aux.img`, `hardware-model.bin`, `machine-id.bin`.
    #[cfg(target_os = "macos")]
    BootMacos {
        /// Guest bundle directory.
        #[arg(long)]
        bundle: std::path::PathBuf,
        /// Output path prefix for screenshots.
        #[arg(long)]
        out_prefix: std::path::PathBuf,
        /// Stop after this many seconds.
        #[arg(long, default_value_t = 90)]
        seconds: u64,
        /// Share a host directory into the guest over virtio-fs, repeatable.
        /// Spec: `TAG=HOSTDIR`. Tag `auto` uses the macOS automount tag, mounting
        /// at `/Volumes/My Shared Files`.
        #[arg(long = "share", value_name = "TAG=HOSTDIR")]
        shares: Vec<String>,
    },
    /// Boot an installed macOS guest fully off-screen and drive it from stdin:
    /// synthetic keyboard/mouse input and on-demand framebuffer screenshots, with
    /// no host cursor or visible window. Reads newline commands
    /// (`key`/`down`/`up`/`type`/`click`/`wait`/`shot`/`quit`) and acks each on
    /// stdout.
    #[cfg(target_os = "macos")]
    DriveMacos {
        /// Guest bundle directory.
        #[arg(long)]
        bundle: std::path::PathBuf,
        /// Share a host directory into the guest over virtio-fs, repeatable.
        /// Spec: `TAG=HOSTDIR`. Tag `auto` uses the macOS automount tag, mounting
        /// at `/Volumes/My Shared Files`.
        #[arg(long = "share", value_name = "TAG=HOSTDIR")]
        shares: Vec<String>,
    },
    /// Copy a nix-built macOS binary and make it guest-portable: repoint every
    /// `/nix/store` dylib to its `/usr/lib` system equivalent (or bundle it next
    /// to the output with an `@loader_path` reference) and ad-hoc re-sign, so the
    /// result links only libraries a vanilla guest has. Verifies that no
    /// `/nix/store` path remains.
    #[cfg(target_os = "macos")]
    StageBinary {
        /// Input binary (typically a `/nix/store` path).
        #[arg(value_name = "IN")]
        input: std::path::PathBuf,
        /// Output path for the staged, guest-portable binary.
        #[arg(value_name = "OUT")]
        output: std::path::PathBuf,
    },
    /// Provision a STOPPED macOS guest bundle so it boots straight past Setup
    /// Assistant to a logged-in desktop. Host-side disk edit: attaches the guest
    /// disk, marks system + per-user setup complete, optionally enables
    /// auto-login, then detaches. Refuses to run if the bundle appears in use.
    #[cfg(target_os = "macos")]
    Provision {
        /// Guest bundle directory (must be stopped).
        #[arg(long)]
        bundle: std::path::PathBuf,
        /// Short name of the guest user whose per-user Setup Assistant to mark
        /// complete (the first account created during install).
        #[arg(long)]
        user: String,
        /// Also enable password-less auto-login for `--user` (writes `kcpassword`
        /// + the loginwindow `autoLoginUser`).
        #[arg(long)]
        autologin: bool,
        /// With `--autologin`, read the user's password from stdin to encode
        /// `kcpassword` (a trailing newline is stripped). Passing it on stdin
        /// rather than as an argument keeps it out of the process table. With no
        /// flag (or no `--autologin`) the password is empty.
        #[arg(long)]
        password_stdin: bool,
    },
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    let cli = Cli::parse();
    // Operating a VM needs an entitlement on the running process:
    // `com.apple.security.virtualization` for the macOS-guest (VZ) paths and
    // `com.apple.security.hypervisor` for the libkrun Linux path. The Nix store
    // binary is unsigned and immutable, so on the first VM command we re-exec a
    // self-signed copy (carrying both) from a per-user cache.
    if !matches!(cli.command, Command::Info)
        && let Err(error) = ensure_signed_and_reexec()
    {
        eprintln!(
            "vmkit: could not self-sign with the virtualization/hypervisor entitlements: {error}"
        );
        return ExitCode::FAILURE;
    }
    match imp::dispatch(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vmkit: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Ad-hoc sign a cached copy of this binary with the virtualization entitlement
/// and re-exec it. Returns `Ok(())` only when already running as the signed copy
/// (sentinel env var set); otherwise it execs and does not return on success.
#[cfg(target_os = "macos")]
fn ensure_signed_and_reexec() -> std::io::Result<()> {
    use std::hash::{Hash, Hasher};
    use std::io::{Error, ErrorKind};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    use std::path::PathBuf;

    const ENTITLEMENTS: &str = include_str!("virtualization.entitlements");

    if std::env::var_os("IX_VMKIT_SIGNED").is_some() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    // Key the cache by the store path so a rebuilt binary is re-signed.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    exe.hash(&mut hasher);
    let key = format!("{:016x}", hasher.finish());

    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| Error::new(ErrorKind::NotFound, "no HOME or XDG_CACHE_HOME"))?;
    let dir = cache_home.join("ix").join("vmkit");
    std::fs::create_dir_all(&dir)?;
    // The cache holds an entitled binary; keep it owner-only.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let signed = dir.join(format!("vmkit-{key}"));

    // Re-sign unless a valid signature already exists (covers a partial/corrupt
    // copy left by a killed run).
    let already_valid = signed.exists()
        && std::process::Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(&signed)
            .status()
            .is_ok_and(|s| s.success());
    if !already_valid {
        // Per-process temp paths so two concurrent first-runs cannot truncate
        // each other's copy mid-codesign; the final rename publishes atomically
        // (last writer wins a byte-identical, validly-signed file).
        let pid = std::process::id();
        let tmp = dir.join(format!("vmkit-{key}.{pid}.tmp"));
        let entitlements = dir.join(format!("virtualization.{pid}.entitlements"));
        std::fs::copy(&exe, &tmp)?;
        std::fs::write(&entitlements, ENTITLEMENTS)?;
        let status = std::process::Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-", "--entitlements"])
            .arg(&entitlements)
            .arg(&tmp)
            .status()?;
        let _ = std::fs::remove_file(&entitlements);
        if !status.success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(Error::other("codesign failed"));
        }
        std::fs::rename(&tmp, &signed)?;
        prune_stale_signed(&dir, &signed);
    }

    Err(std::process::Command::new(&signed)
        .env("IX_VMKIT_SIGNED", "1")
        .args(std::env::args_os().skip(1))
        .exec())
}

/// Remove signed copies from prior store paths so the cache does not grow with
/// every rebuild. Leaves any in-progress `.tmp` (another process may be writing
/// it) and the copy we just published.
#[cfg(target_os = "macos")]
fn prune_stale_signed(dir: &std::path::Path, keep: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("vmkit-") && !name.ends_with(".tmp") && path != keep {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Linux host: no entitlement/self-sign step (classic libkrun talks to `/dev/kvm`
/// directly). Dispatch the two host-relevant subcommands; the macOS-guest paths
/// do not exist in the `Command` enum on Linux.
#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch_linux(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("vmkit: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "linux")]
fn dispatch_linux(command: Command) -> Result<(), linuxkrun::Error> {
    use std::time::Duration;
    match command {
        Command::Info => {
            // KVM is the libkrun backend on Linux; its presence is the host check.
            let supported = std::path::Path::new("/dev/kvm").exists();
            println!("virtualization_supported={supported}");
            if supported {
                Ok(())
            } else {
                Err(linuxkrun::Error::Setup {
                    message: "/dev/kvm is not present; libkrun needs KVM on Linux".to_owned(),
                })
            }
        }
        Command::BootLinux {
            root,
            exec,
            gpu,
            cpus,
            memory_mib,
            ports,
            net,
            vsock_ports,
            console_file,
            timeout_secs,
        } => {
            let net =
                build_net(net, &ports).map_err(|message| linuxkrun::Error::Setup { message })?;
            let vsock_ports = build_vsock_ports(&vsock_ports)
                .map_err(|message| linuxkrun::Error::Setup { message })?;
            let timeout = (timeout_secs != 0).then(|| Duration::from_secs(timeout_secs));
            linuxkrun::boot_linux(&linuxkrun::BootLinux {
                root,
                exec,
                gpu,
                cpus,
                memory_mib,
                net,
                vsock_ports,
                console_file,
                timeout,
            })
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn main() -> ExitCode {
    eprintln!("vmkit: requires macOS (Virtualization.framework) or Linux (libkrun/KVM)");
    ExitCode::FAILURE
}

/// Build the optional guest network from the `--net` / `--port` flags. `--net`
/// (or any `--port`) attaches a guest NIC with outbound access; each `--port`
/// `HOST:GUEST` becomes a host->guest TCP forward. Returns `Ok(None)` when
/// neither is given (no network). The `String` error is a user-facing CLI
/// message the caller wraps in its host error type.
fn build_net(net: bool, ports: &[String]) -> Result<Option<net::Net>, String> {
    if !net && ports.is_empty() {
        return Ok(None);
    }
    let mut forwards = Vec::with_capacity(ports.len());
    for spec in ports {
        let (host, guest) = spec
            .split_once(':')
            .ok_or_else(|| format!("--port {spec:?} must be HOST:GUEST"))?;
        let host: u16 = host
            .parse()
            .map_err(|_| format!("--port {spec:?}: invalid host port"))?;
        let guest: u16 = guest
            .parse()
            .map_err(|_| format!("--port {spec:?}: invalid guest port"))?;
        forwards.push(net::Forward { host, guest });
    }
    Ok(Some(net::Net { forwards }))
}

/// Parse `--vsock-port GUEST_PORT:HOST_PATH` specs into [`linuxkrun::VsockPort`]
/// mappings for [`linuxkrun::BootLinux::vsock_ports`]. Split at the *first* `:`
/// only: a unix path is free to contain `:`. The `String` error is a user-facing
/// CLI message the caller wraps in its host error type, as in [`build_net`].
fn build_vsock_ports(specs: &[String]) -> Result<Vec<linuxkrun::VsockPort>, String> {
    let mut ports = Vec::with_capacity(specs.len());
    for spec in specs {
        let (port, path) = spec
            .split_once(':')
            .ok_or_else(|| format!("--vsock-port {spec:?} must be GUEST_PORT:HOST_PATH"))?;
        let port: u32 = port
            .parse()
            .map_err(|_| format!("--vsock-port {spec:?}: invalid guest port"))?;
        // Port 0 and VMADDR_PORT_ANY are reserved; reject here with a legible
        // message instead of deferring to a late libkrun -errno at boot.
        if port == 0 || port == u32::MAX {
            return Err(format!("--vsock-port {spec:?}: guest port must be 1..u32::MAX-1"));
        }
        if path.is_empty() {
            return Err(format!("--vsock-port {spec:?} has an empty host socket path"));
        }
        ports.push(linuxkrun::VsockPort {
            guest_port: port,
            host_path: std::path::PathBuf::from(path),
        });
    }
    Ok(ports)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::build_vsock_ports;
    use super::linuxkrun::VsockPort;

    #[test]
    fn vsock_port_spec_parses_port_and_path() {
        let specs = [String::from("7100:/tmp/guest.sock")];
        let ports = build_vsock_ports(&specs).expect("a valid spec parses");
        assert_eq!(
            ports,
            [VsockPort { guest_port: 7100, host_path: PathBuf::from("/tmp/guest.sock") }]
        );
    }

    #[test]
    fn vsock_port_path_keeps_later_colons() {
        // Only the first `:` separates port from path; the rest is the path.
        let specs = [String::from("7100:/tmp/a:b.sock")];
        let ports = build_vsock_ports(&specs).expect("a colon in the path is legal");
        assert_eq!(
            ports,
            [VsockPort { guest_port: 7100, host_path: PathBuf::from("/tmp/a:b.sock") }]
        );
    }

    #[test]
    fn vsock_port_rejects_malformed_specs() {
        for bad in ["7100", "nope:/tmp/x.sock", "7100:", "-1:/tmp/x.sock", "0:/tmp/x.sock", "4294967295:/tmp/x.sock"] {
            let specs = [String::from(bad)];
            assert!(build_vsock_ports(&specs).is_err(), "{bad:?} must be rejected");
        }
    }
}

// The libkrun backend compiles on every host (its internals are cfg-split, and a
// host without libkrun gets a typed stub); the Apple-framework modules are macOS
// only.
mod linuxkrun;

// Guest networking (gvproxy on a macOS host, TSI port-map on Linux). Compiles on
// every host; the gvproxy `Proxy` is macOS-only inside.
mod net;

#[cfg(target_os = "macos")]
mod drive;

#[cfg(target_os = "macos")]
mod input;

#[cfg(target_os = "macos")]
mod linuxguest;

#[cfg(target_os = "macos")]
mod macguest;

#[cfg(target_os = "macos")]
mod provision;

#[cfg(target_os = "macos")]
mod stagebin;

#[cfg(target_os = "macos")]
mod imp {
    //! Command dispatch and the pieces shared across the macOS backends: the
    //! crate-wide [`Error`], `file_url`/`ns_error_message` helpers (used by
    //! [`crate::macguest`] and [`crate::linuxguest`]), and `info`. The
    //! VM-creation glue lives in the per-guest backends: macOS guests in
    //! [`crate::macguest`]/[`crate::drive`] (Virtualization.framework), Linux
    //! guests in [`crate::linuxkrun`] (libkrun).

    use std::path::PathBuf;
    use std::time::Duration;

    use objc2::rc::Retained;
    use objc2_foundation::{NSError, NSString, NSURL};
    use objc2_virtualization::VZVirtualMachine;
    use snafu::Snafu;

    use crate::Command;

    #[derive(Debug, Snafu)]
    pub enum Error {
        #[snafu(display("virtualization is not available on this host"))]
        Unsupported,
        #[snafu(display("{message}"))]
        Args { message: String },
        #[snafu(display("guest memory {mib} MiB is too large to express in bytes"))]
        MemoryTooLarge { mib: u64 },
        #[snafu(display(
            "virtual machine configuration is invalid: {message} \
             (an unsigned binary, or one missing com.apple.security.virtualization, \
             fails configuration validation)"
        ))]
        InvalidConfiguration { message: String },
        #[snafu(display("must run on the main thread"))]
        NotMainThread,
        #[snafu(display("guest framebuffer not available yet"))]
        NoFramebuffer,
        #[snafu(display("invalid guest bundle: {message}"))]
        Bundle { message: String },
        #[snafu(display("screenshot encode/write failed: {message}"))]
        CaptureEncode { message: String },
        #[snafu(display("staging the binary guest-portable failed: {source}"))]
        StageBinary { source: crate::stagebin::Error },
        #[snafu(display("provisioning the guest failed: {source}"))]
        Provision { source: crate::provision::Error },
        #[snafu(display("{source}"))]
        Linux { source: crate::linuxkrun::Error },
    }

    pub fn dispatch(command: Command) -> Result<(), Error> {
        match command {
            Command::Info => info(),
            Command::BootLinux {
                disks,
                gpu,
                cpus,
                memory_mib,
                ports,
                net,
                vsock_ports,
                console_file,
                timeout_secs,
            } => {
                let net =
                    crate::build_net(net, &ports).map_err(|message| Error::Args { message })?;
                let vsock_ports = crate::build_vsock_ports(&vsock_ports)
                    .map_err(|message| Error::Args { message })?;
                let timeout = (timeout_secs != 0).then(|| Duration::from_secs(timeout_secs));
                crate::linuxkrun::boot_linux(&crate::linuxkrun::BootLinux {
                    disks,
                    gpu,
                    cpus,
                    memory_mib,
                    net,
                    vsock_ports,
                    console_file,
                    timeout,
                })
                .map_err(|source| Error::Linux { source })
            }
            Command::BootLinuxGui {
                disk,
                efi_vars,
                out_prefix,
                seconds,
                cpus,
                memory_mib,
            } => {
                // Default the EFI variable store next to the disk so a bare
                // `--disk foo.raw` just works and persists boot entries.
                let var_store = efi_vars.unwrap_or_else(|| disk.with_extension("efivars"));
                crate::linuxguest::boot_linux_gui(crate::linuxguest::LinuxGuiBoot {
                    disk,
                    var_store,
                    out_prefix,
                    seconds,
                    cpus,
                    memory_mib,
                })
            }
            Command::DriveLinux {
                disk,
                efi_vars,
                cpus,
                memory_mib,
            } => {
                let var_store = efi_vars.unwrap_or_else(|| disk.with_extension("efivars"));
                crate::linuxguest::drive_linux(crate::linuxguest::DriveLinux {
                    disk,
                    var_store,
                    cpus,
                    memory_mib,
                })
            }
            Command::InstallMacos {
                ipsw,
                bundle,
                disk_gib,
            } => crate::macguest::install_macos(ipsw, bundle, disk_gib),
            Command::BootMacos {
                bundle,
                out_prefix,
                seconds,
                shares,
            } => crate::macguest::boot_macos_screenshot(crate::macguest::MacBootScreenshot {
                bundle,
                out_prefix,
                seconds,
                shares: parse_shares(&shares)?,
            }),
            Command::DriveMacos { bundle, shares } => {
                crate::drive::drive_macos(crate::drive::DriveMacos {
                    bundle,
                    shares: parse_shares(&shares)?,
                })
            }
            Command::StageBinary { input, output } => {
                let staged = crate::stagebin::stage_binary(&input, &output)
                    .map_err(|source| Error::StageBinary { source })?;
                println!("{}", staged.display());
                Ok(())
            }
            Command::Provision {
                bundle,
                user,
                autologin,
                password_stdin,
            } => {
                // Read the autologin password from stdin (never an argument, so it
                // stays out of the process table). Only when both --autologin and
                // --password-stdin are set; otherwise the password is empty.
                let password = if autologin && password_stdin {
                    read_password_stdin()?
                } else {
                    String::new()
                };
                crate::provision::provision(crate::provision::Provision {
                    bundle,
                    user,
                    autologin,
                    password,
                })
                .map_err(|source| Error::Provision { source })
            }
        }
    }

    /// Read a password from stdin, stripping a single trailing newline (and an
    /// accompanying CR). The whole of stdin is the password, so a passphrase
    /// containing spaces or other shell-significant bytes is passed verbatim.
    fn read_password_stdin() -> Result<String, Error> {
        use std::io::Read;
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| Error::Bundle {
                message: format!("read password from stdin: {e}"),
            })?;
        if let Some(stripped) = buf.strip_suffix('\n') {
            buf.truncate(stripped.len());
            if let Some(stripped) = buf.strip_suffix('\r') {
                buf.truncate(stripped.len());
            }
        }
        Ok(buf)
    }

    /// Parse `--share TAG=HOSTDIR` specs into [`DirShare`]s. Tag `auto` maps to
    /// the macOS automount tag. Shares are read-write; a read-only option, if
    /// needed, should be a dedicated flag rather than an in-path suffix (a Unix
    /// path is free to contain any separator, so overloading `HOSTDIR` would be
    /// ambiguous).
    fn parse_shares(specs: &[String]) -> Result<Vec<crate::macguest::DirShare>, Error> {
        use crate::macguest::{DirShare, ShareTag};

        specs
            .iter()
            .map(|spec| {
                let (tag, dir) = spec.split_once('=').ok_or_else(|| Error::Bundle {
                    message: format!("share {spec:?} must be TAG=HOSTDIR"),
                })?;
                if dir.is_empty() {
                    return Err(Error::Bundle {
                        message: format!("share {spec:?} has an empty host directory"),
                    });
                }
                let tag = if tag == "auto" {
                    ShareTag::Automount
                } else {
                    ShareTag::Named(tag.to_owned())
                };
                Ok(DirShare {
                    tag,
                    host_dir: PathBuf::from(dir),
                })
            })
            .collect()
    }

    fn info() -> Result<(), Error> {
        let supported = unsafe { VZVirtualMachine::isSupported() };
        println!("virtualization_supported={supported}");
        if supported {
            Ok(())
        } else {
            Err(Error::Unsupported)
        }
    }

    pub fn file_url(path: &std::path::Path) -> Retained<NSURL> {
        let s = NSString::from_str(&path.to_string_lossy());
        NSURL::fileURLWithPath(&s)
    }

    pub fn ns_error_message(error: &NSError) -> String {
        error.localizedDescription().to_string()
    }
}
