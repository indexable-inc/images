//! Linux guest backend: boot a Linux VM via **libkrun** (Hypervisor.framework),
//! not Virtualization.framework.
//!
//! This is the Linux half of the crate's generic split: macOS guests run on
//! Virtualization.framework ([`crate::macguest`]), Linux guests run on libkrun.
//! libkrun is the only path that gives a Linux guest GPU acceleration on Apple
//! Silicon (virtio-gpu/Venus -> MoltenVK -> Metal); VZ exposes no GPU to Linux
//! guests. See the README and `docs/linux-libkrun.md`.
//!
//! We link the macOS-only **libkrun-efi** variant (`-lkrun`, which resolves to
//! `libkrun-efi.dylib`). Classic libkrun is Linux-only in nixpkgs (its
//! `libkrunfw` kernel package does not build on Darwin), so the EFI variant is
//! the only libkrun available here. The EFI build always boots its embedded
//! OVMF/EDK2 firmware (`krun_set_kernel` is ignored), so a guest is a raw
//! EFI-bootable disk image, the same disk shape `VZEFIBootLoader` takes.
//!
//! The OVMF firmware blob lives in the libkrun source tree and is embedded into
//! this binary at build time (`KRUN_EFI_FIRMWARE`, set by the nix build to
//! `${libkrun-efi.src}/edk2/KRUN_EFI.silent.fd`). `krun_set_firmware` wants a
//! path, so the embedded bytes are written to a per-user cache file at runtime;
//! embedding keeps the binary self-contained across the entitlement self-sign
//! re-exec (see `main.rs`), which would otherwise break a relative firmware
//! lookup.

use std::ffi::CString;
use std::os::raw::c_char;
use std::path::PathBuf;
use std::time::Duration;

use crate::imp::Error;

// libkrun C API (libkrun.h). Hand-written rather than via the `krun-sys` crate,
// which is stale (1.10.x vs libkrun 1.18) and EFI-blind. Every call returns
// `i32`: >= 0 success, negative is `-errno`. `#[link(name = "krun")]` resolves
// to `libkrun-efi.dylib` through the symlink chain the nix package provides; the
// search path and rpath come from the workspace build (see lib/rust/workspace.nix).
#[link(name = "krun")]
unsafe extern "C" {
    fn krun_set_log_level(level: u32) -> i32;
    fn krun_create_ctx() -> i32;
    fn krun_set_vm_config(ctx_id: u32, num_vcpus: u8, ram_mib: u32) -> i32;
    fn krun_set_firmware(ctx_id: u32, firmware_path: *const c_char) -> i32;
    fn krun_add_disk2(
        ctx_id: u32,
        block_id: *const c_char,
        disk_path: *const c_char,
        disk_format: u32,
        read_only: bool,
    ) -> i32;
    fn krun_set_gpu_options2(ctx_id: u32, virgl_flags: u32, shm_size: u64) -> i32;
    fn krun_set_console_output(ctx_id: u32, c_filepath: *const c_char) -> i32;
    fn krun_start_enter(ctx_id: u32) -> i32;
}

const KRUN_DISK_FORMAT_RAW: u32 = 0;
// virtio-gpu Venus, no virgl: the flag set krunkit uses on macOS for a
// MoltenVK-backed Vulkan device (libkrun.h VIRGLRENDERER_VENUS | _NO_VIRGL).
const VIRGLRENDERER_VENUS: u32 = 1 << 6;
const VIRGLRENDERER_NO_VIRGL: u32 = 1 << 7;
// GPU shared-memory (vRAM) window. 1 GiB matches krunkit's default.
const GPU_SHM_BYTES: u64 = 1 << 30;

/// The OVMF/EDK2 firmware the EFI guest boots, embedded at build time so the
/// binary is self-contained (survives the self-sign re-exec).
const FIRMWARE: &[u8] = include_bytes!(env!("KRUN_EFI_FIRMWARE"));

/// Parameters for booting a Linux guest under libkrun. Named fields (like
/// [`crate::imp`]'s boot structs) so callers and a future IPC layer name each.
pub struct BootLinux {
    /// Raw EFI-bootable disk image (e.g. a NixOS `raw-efi` image, or a Fedora
    /// CoreOS raw). The guest's own bootloader/kernel live in it; OVMF boots it.
    pub disk: PathBuf,
    /// Enable a virtio-gpu Venus device (GPU acceleration via MoltenVK). The
    /// guest gets `/dev/dri/renderD128`. VZ cannot provide this.
    pub gpu: bool,
    pub cpus: u8,
    pub memory_mib: u32,
    /// Capture the guest serial console to this file instead of inheriting the
    /// process stdio. `krun_start_enter` takes over the process, so a file is
    /// how a background/lockstep caller reads the console after the VM stops.
    pub console_file: Option<PathBuf>,
    /// Stop the VM and exit after this long (a watchdog, so a background
    /// invocation never hangs).
    pub timeout: Duration,
}

fn cstr(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error::Bundle {
        message: format!("path {s:?} contains a NUL byte"),
    })
}

/// Write the embedded firmware to a per-user cache file (once) and return its
/// path, since `krun_set_firmware` takes a path, not bytes.
fn firmware_path() -> Result<PathBuf, Error> {
    use std::os::unix::fs::PermissionsExt;

    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| Error::Bundle {
            message: "no HOME or XDG_CACHE_HOME for firmware cache".to_owned(),
        })?
        .join("ix")
        .join("macos-vm");
    std::fs::create_dir_all(&cache).map_err(|e| Error::Bundle {
        message: format!("create firmware cache dir: {e}"),
    })?;
    // Owner-only, matching the signed-binary cache in `main.rs`.
    let _ = std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700));
    // Name by content length so a firmware change lands a fresh file; the blob
    // is immutable for a given build.
    let path = cache.join(format!("KRUN_EFI.silent.{}.fd", FIRMWARE.len()));
    let fresh = std::fs::metadata(&path).map(|m| m.len() as usize).ok() == Some(FIRMWARE.len());
    if !fresh {
        // Atomic publish: write a per-process temp then rename, so a concurrent
        // (or killed mid-write) run never leaves libkrun reading a torn firmware
        // that the length-only freshness check would not re-detect.
        let tmp = cache.join(format!(
            "KRUN_EFI.silent.{}.{}.tmp",
            FIRMWARE.len(),
            std::process::id()
        ));
        std::fs::write(&tmp, FIRMWARE).map_err(|e| Error::Bundle {
            message: format!("write firmware: {e}"),
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| Error::Bundle {
            message: format!("publish firmware: {e}"),
        })?;
    }
    Ok(path)
}

fn check(op: &str, code: i32) -> Result<(), Error> {
    if code >= 0 {
        Ok(())
    } else {
        Err(Error::Libkrun {
            op: op.to_owned(),
            code,
        })
    }
}

/// Boot a Linux guest under libkrun and run it until it powers off or the
/// timeout elapses. Does not return on success: `krun_start_enter` takes over
/// the process and `exit()`s with the guest's exit code when the VM stops.
pub fn boot_linux(boot: BootLinux) -> Result<(), Error> {
    if !boot.disk.exists() {
        return Err(Error::Disk {
            path: boot.disk.clone(),
            message: "disk image does not exist".to_owned(),
        });
    }
    let firmware = firmware_path()?;
    let disk = boot
        .disk
        .canonicalize()
        .unwrap_or_else(|_| boot.disk.clone());

    // Strings must outlive the krun calls that borrow their pointers.
    let firmware_c = cstr(&firmware.to_string_lossy())?;
    let disk_c = cstr(&disk.to_string_lossy())?;
    let block_id = cstr("root")?;
    let console_c = boot
        .console_file
        .as_ref()
        .map(|p| cstr(&p.to_string_lossy()))
        .transpose()?;

    // The watchdog ends the process if the guest hangs; the console has streamed
    // by then. Spawned before krun_start_enter, which never returns.
    let timeout = boot.timeout;
    std::thread::spawn(move || {
        std::thread::sleep(timeout);
        eprintln!("macos-vm: timeout reached, stopping");
        std::process::exit(0);
    });

    // Safety: every pointer passed below outlives its call (the CStrings live to
    // the end of this function, and krun copies what it needs); krun_start_enter
    // consumes the context.
    unsafe {
        check("krun_set_log_level", krun_set_log_level(2))?; // warn
        let ctx = krun_create_ctx();
        check("krun_create_ctx", ctx)?;
        let ctx = ctx as u32;
        check(
            "krun_set_vm_config",
            krun_set_vm_config(ctx, boot.cpus, boot.memory_mib),
        )?;
        check(
            "krun_set_firmware",
            krun_set_firmware(ctx, firmware_c.as_ptr()),
        )?;
        check(
            "krun_add_disk2",
            krun_add_disk2(
                ctx,
                block_id.as_ptr(),
                disk_c.as_ptr(),
                KRUN_DISK_FORMAT_RAW,
                false,
            ),
        )?;
        if boot.gpu {
            check(
                "krun_set_gpu_options2",
                krun_set_gpu_options2(
                    ctx,
                    VIRGLRENDERER_VENUS | VIRGLRENDERER_NO_VIRGL,
                    GPU_SHM_BYTES,
                ),
            )?;
        }
        if let Some(console) = &console_c {
            check(
                "krun_set_console_output",
                krun_set_console_output(ctx, console.as_ptr()),
            )?;
        }
        // Never returns on success: takes over the process, exit()s with the
        // guest exit code when the VM powers off. A negative return is a
        // pre-boot config error.
        let code = krun_start_enter(ctx);
        Err(Error::Libkrun {
            op: "krun_start_enter".to_owned(),
            code,
        })
    }
}
