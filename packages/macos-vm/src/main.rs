//! `macos-vm`: own a VM's lifecycle from Rust, one backend per guest OS.
//!
//! macOS guests run on Apple's Virtualization.framework ([`macguest`],
//! [`drive`]); Linux guests run on libkrun ([`linuxkrun`], Hypervisor.framework),
//! the only backend that gives a Linux guest a GPU on Apple Silicon. The binary
//! owns the VM so callers that cannot hold the entitlements themselves (notably
//! the ix-mcp Python interpreter, an unsigned immutable Nix store binary) can
//! spawn it and control a VM over IPC. The `com.apple.security.virtualization`
//! and `com.apple.security.hypervisor` entitlements live on *this* signed
//! process, never on the interpreter (see [`ensure_signed_and_reexec`]).
//!
//! `macos-vm info` reports whether virtualization is available. `macos-vm
//! boot-linux --disk <raw-efi-disk> [--gpu]` boots a Linux guest under libkrun
//! and streams its serial console; this is the end-to-end smoke path (a real
//! guest reaching userspace proves the binding, the entitlement, and the boot).
//! The macOS-guest, GUI-capture, and provisioning paths are tracked in the
//! README and `docs/linux-libkrun.md`.
//!
//! Off macOS the binary builds (so the Linux CI workspace graph stays green) but
//! is a single typed refusal: all VZ/libkrun code lives in the
//! `cfg(target_os = "macos")` modules below, so the Linux compile sees only the
//! `main` at the bottom of this file.

use std::process::ExitCode;

#[cfg(target_os = "macos")]
use clap::{Parser, Subcommand};

#[cfg(target_os = "macos")]
#[derive(Debug, Parser)]
#[command(
    name = "macos-vm",
    about = "Drive Apple's Virtualization.framework from Rust"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Subcommand)]
enum Command {
    /// Report whether Virtualization.framework can run a VM on this host.
    Info,
    /// Boot a Linux guest from a raw EFI-bootable disk via libkrun
    /// (Hypervisor.framework), streaming its serial console until the guest
    /// powers off or the timeout elapses. Unlike the macOS-guest paths this does
    /// not use Virtualization.framework: libkrun is the only backend that can
    /// give a Linux guest GPU acceleration on Apple Silicon (`--gpu`).
    BootLinux {
        /// Path to a raw EFI-bootable disk image (e.g. a NixOS `raw-efi` image
        /// or a Fedora CoreOS raw). The guest's own kernel/bootloader live in
        /// it; libkrun's embedded OVMF firmware boots it.
        #[arg(long)]
        disk: std::path::PathBuf,
        /// Enable a virtio-gpu Venus device so the guest gets a real GPU
        /// (`/dev/dri/renderD128`, Vulkan via MoltenVK). Off by default.
        #[arg(long)]
        gpu: bool,
        /// Number of virtual CPUs.
        #[arg(long, default_value_t = 2)]
        cpus: usize,
        /// Guest memory in MiB.
        #[arg(long, default_value_t = 1024)]
        memory_mib: u64,
        /// Capture the guest serial console to this file instead of the
        /// process's stdout (useful for a background/lockstep caller).
        #[arg(long)]
        console_file: Option<std::path::PathBuf>,
        /// Stop the VM and exit after this many seconds.
        #[arg(long, default_value_t = 20)]
        timeout_secs: u64,
    },
    /// Boot an aarch64 Linux GUI guest from a raw EFI disk with a virtio-gpu
    /// display + USB keyboard/mouse, fully off-screen, and screenshot its
    /// framebuffer to `<out-prefix>.NNN.png` (no window, host cursor untouched).
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
    /// Boot an aarch64 Linux GUI guest from a raw EFI disk off-screen and drive
    /// it from stdin: synthetic keyboard/mouse and on-demand framebuffer
    /// screenshots, with no host cursor or visible window. Same newline command
    /// protocol as `drive-macos` (`key`/`down`/`up`/`type`/`click`/`wait`/`shot`/`quit`).
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
    /// Screen-Recording permission). The bundle is a directory with
    /// `disk.img`, `aux.img`, `hardware-model.bin`, `machine-id.bin`.
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
        /// Spec: `TAG=HOSTDIR`. Tag `auto` uses the macOS automount tag,
        /// mounting at `/Volumes/My Shared Files`.
        #[arg(long = "share", value_name = "TAG=HOSTDIR")]
        shares: Vec<String>,
    },
    /// Boot an installed macOS guest fully off-screen and drive it from stdin:
    /// synthetic keyboard/mouse input and on-demand framebuffer screenshots,
    /// with no host cursor or visible window. Reads newline commands
    /// (`key`/`down`/`up`/`type`/`click`/`wait`/`shot`/`quit`) and acks each on
    /// stdout.
    DriveMacos {
        /// Guest bundle directory.
        #[arg(long)]
        bundle: std::path::PathBuf,
        /// Share a host directory into the guest over virtio-fs, repeatable.
        /// Spec: `TAG=HOSTDIR`. Tag `auto` uses the macOS automount tag,
        /// mounting at `/Volumes/My Shared Files`.
        #[arg(long = "share", value_name = "TAG=HOSTDIR")]
        shares: Vec<String>,
    },
    /// Copy a nix-built macOS binary and make it guest-portable: repoint every
    /// `/nix/store` dylib to its `/usr/lib` system equivalent (or bundle it
    /// next to the output with an `@loader_path` reference) and ad-hoc re-sign,
    /// so the result links only libraries a vanilla guest has. Verifies that no
    /// `/nix/store` path remains.
    StageBinary {
        /// Input binary (typically a `/nix/store` path).
        #[arg(value_name = "IN")]
        input: std::path::PathBuf,
        /// Output path for the staged, guest-portable binary.
        #[arg(value_name = "OUT")]
        output: std::path::PathBuf,
    },
    /// Provision a STOPPED macOS guest bundle so it boots straight past Setup
    /// Assistant to a logged-in desktop. Host-side disk edit: attaches the
    /// guest disk, marks system + per-user setup complete, optionally enables
    /// auto-login, then detaches. Refuses to run if the bundle appears in use.
    Provision {
        /// Guest bundle directory (must be stopped).
        #[arg(long)]
        bundle: std::path::PathBuf,
        /// Short name of the guest user whose per-user Setup Assistant to mark
        /// complete (the first account created during install).
        #[arg(long)]
        user: String,
        /// Also enable password-less auto-login for `--user` (writes
        /// `kcpassword` + the loginwindow `autoLoginUser`).
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
            "macos-vm: could not self-sign with the virtualization/hypervisor entitlements: {error}"
        );
        return ExitCode::FAILURE;
    }
    match imp::dispatch(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("macos-vm: {error}");
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

    if std::env::var_os("IX_MACVM_SIGNED").is_some() {
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
    let dir = cache_home.join("ix").join("macos-vm");
    std::fs::create_dir_all(&dir)?;
    // The cache holds an entitled binary; keep it owner-only.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let signed = dir.join(format!("macos-vm-{key}"));

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
        let tmp = dir.join(format!("macos-vm-{key}.{pid}.tmp"));
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
        .env("IX_MACVM_SIGNED", "1")
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
        if name.starts_with("macos-vm-") && !name.ends_with(".tmp") && path != keep {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    eprintln!("macos-vm: requires macOS and Apple's Virtualization.framework");
    ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
mod drive;

#[cfg(target_os = "macos")]
mod input;

#[cfg(target_os = "macos")]
mod linuxguest;

#[cfg(target_os = "macos")]
mod linuxkrun;

#[cfg(target_os = "macos")]
mod macguest;

#[cfg(target_os = "macos")]
mod provision;

#[cfg(target_os = "macos")]
mod stagebin;

#[cfg(target_os = "macos")]
mod imp {
    //! Command dispatch and the pieces shared across backends: the crate-wide
    //! [`Error`], `file_url`/`ns_error_message` helpers (used by [`crate::macguest`]
    //! and [`crate::linuxguest`]), and `info`. The VM-creation glue lives in the
    //! per-guest backends: macOS guests in [`crate::macguest`]/[`crate::drive`]
    //! (Virtualization.framework), Linux guests in [`crate::linuxkrun`] (libkrun).

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
        #[snafu(display("cannot attach disk {path:?}: {message}"))]
        Disk { path: PathBuf, message: String },
        #[snafu(display("screenshot encode/write failed: {message}"))]
        CaptureEncode { message: String },
        #[snafu(display("staging the binary guest-portable failed: {source}"))]
        StageBinary { source: crate::stagebin::Error },
        #[snafu(display("provisioning the guest failed: {source}"))]
        Provision { source: crate::provision::Error },
        #[snafu(display(
            "libkrun {op} failed with code {code} (negative is -errno); an \
             unsigned binary, or one missing com.apple.security.hypervisor, \
             cannot create a VM"
        ))]
        Libkrun { op: String, code: i32 },
    }

    pub fn dispatch(command: Command) -> Result<(), Error> {
        match command {
            Command::Info => info(),
            Command::BootLinux {
                disk,
                gpu,
                cpus,
                memory_mib,
                console_file,
                timeout_secs,
            } => crate::linuxkrun::boot_linux(crate::linuxkrun::BootLinux {
                disk,
                gpu,
                // libkrun caps vCPUs at 16 and RAM well under u32 MiB; the wider
                // CLI types just narrow here.
                cpus: cpus.min(u8::MAX as usize) as u8,
                memory_mib: memory_mib.min(u32::MAX as u64) as u32,
                console_file,
                timeout: Duration::from_secs(timeout_secs),
            }),
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
                // Read the autologin password from stdin (never an argument, so
                // it stays out of the process table). Only when both --autologin
                // and --password-stdin are set; otherwise the password is empty.
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
