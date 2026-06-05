//! Linux guest backend via **libkrun**. Compiles on both hosts; the boot model
//! differs by host, so only the payload step is cfg-split.
//!
//! - **macOS host**: links the `libkrun-efi` variant (Hypervisor.framework).
//!   classic libkrun is Linux-only in nixpkgs (its `libkrunfw` kernel does not
//!   build on Darwin), so EFI is the only libkrun here. The EFI build always
//!   boots its embedded OVMF/EDK2 firmware (`krun_set_kernel`/`krun_set_root`
//!   are ignored), so a guest is a raw EFI-bootable disk carrying its own
//!   kernel/bootloader, the same disk shape `VZEFIBootLoader` takes. The firmware
//!   blob is embedded at build time (`KRUN_EFI_FIRMWARE`) and written to a
//!   per-user cache because `krun_set_firmware` wants a path; embedding keeps the
//!   binary self-contained across the entitlement self-sign re-exec (see `main.rs`).
//! - **Linux host**: links classic `libkrun` (KVM). Its bundled `libkrunfw`
//!   kernel boots a rootfs directory shared in over virtiofs (`krun_set_root`)
//!   and runs an exec command as the guest init (`krun_set_exec`), the same model
//!   `podman --runtime krun` / `crun` use. No firmware, no guest-supplied kernel.
//!
//! libkrun is also the only path that gives a Linux guest GPU acceleration on
//! Apple Silicon (virtio-gpu/Venus -> `MoltenVK` -> Metal); VZ exposes no GPU to
//! Linux guests. See the README and `docs/linux-libkrun.md`.
//!
//! Both hosts link `-lkrun` (the nix build resolves it to the right dylib/so and
//! supplies the search path + rpath, see lib/rust/workspace.nix) and share the
//! ctx/config/gpu/console/watchdog/`krun_start_enter` skeleton. The whole libkrun
//! surface is gated on the `have_libkrun` cfg, which the build script sets only
//! when libkrun is linkable for the build host (native aarch64-darwin, or a Linux
//! host). A build without it (e.g. a Linux->darwin cross build) compiles the stub
//! `boot_linux` at the bottom.

use std::path::PathBuf;
use std::time::Duration;

#[cfg(have_libkrun)]
use std::ffi::CString;
#[cfg(have_libkrun)]
use std::os::raw::c_char;

use snafu::Snafu;

/// Errors from the libkrun backend. Self-contained (no Apple types) so the module
/// compiles on a Linux host as well as macOS.
#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("cannot use {path:?}: {message}"))]
    Source { path: PathBuf, message: String },
    #[snafu(display("path {path:?} contains a NUL byte"))]
    Nul { path: String },
    #[snafu(display("{message}"))]
    Setup { message: String },
    #[snafu(display(
        "libkrun {op} failed with code {code} (negative is -errno); the host may be \
         unable to create a VM (a macOS binary missing com.apple.security.hypervisor, \
         or a Linux host without access to /dev/kvm)"
    ))]
    Libkrun { op: String, code: i32 },
    // Only the stub `boot_linux` constructs this; gate it to the same cfg so a
    // `have_libkrun` build (where the stub is absent) does not carry an unused
    // variant (a binary crate lints unused enum variants as dead code).
    #[cfg(not(have_libkrun))]
    #[snafu(display(
        "boot-linux: the libkrun backend was not built into this binary \
         (it links libkrun only on a native aarch64-darwin or Linux build)"
    ))]
    NotBuilt,
}

// libkrun C API (libkrun.h). Hand-written rather than via the `krun-sys` crate,
// which is stale (1.10.x vs libkrun 1.18) and EFI-blind. Every call returns
// `i32`: >= 0 success, negative is `-errno`. `#[link(name = "krun")]` resolves to
// `libkrun-efi.dylib` (macOS) or `libkrun.so` (Linux) through the nix package; the
// search path and rpath come from the workspace build (see lib/rust/workspace.nix).
#[cfg(have_libkrun)]
#[link(name = "krun")]
unsafe extern "C" {
    fn krun_set_log_level(level: u32) -> i32;
    fn krun_create_ctx() -> i32;
    fn krun_set_vm_config(ctx_id: u32, num_vcpus: u8, ram_mib: u32) -> i32;
    fn krun_set_gpu_options2(ctx_id: u32, virgl_flags: u32, shm_size: u64) -> i32;
    fn krun_set_console_output(ctx_id: u32, c_filepath: *const c_char) -> i32;
    fn krun_start_enter(ctx_id: u32) -> i32;

    // macOS / libkrun-efi: boot an EFI disk under the embedded OVMF firmware.
    #[cfg(target_os = "macos")]
    fn krun_set_firmware(ctx_id: u32, firmware_path: *const c_char) -> i32;
    #[cfg(target_os = "macos")]
    fn krun_add_disk2(
        ctx_id: u32,
        block_id: *const c_char,
        disk_path: *const c_char,
        disk_format: u32,
        read_only: bool,
    ) -> i32;

    // Linux / classic libkrun: boot the bundled kernel against a rootfs dir and
    // run an exec command as the guest init.
    #[cfg(target_os = "linux")]
    fn krun_set_root(ctx_id: u32, root_path: *const c_char) -> i32;
    #[cfg(target_os = "linux")]
    fn krun_set_workdir(ctx_id: u32, workdir_path: *const c_char) -> i32;
    #[cfg(target_os = "linux")]
    fn krun_set_exec(
        ctx_id: u32,
        exec_path: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
    ) -> i32;
}

#[cfg(all(have_libkrun, target_os = "macos"))]
const KRUN_DISK_FORMAT_RAW: u32 = 0;
// virtio-gpu Venus, no virgl: the flag set krunkit uses for a MoltenVK-backed
// Vulkan device (libkrun.h VIRGLRENDERER_VENUS | _NO_VIRGL).
#[cfg(have_libkrun)]
const VIRGLRENDERER_VENUS: u32 = 1 << 6;
#[cfg(have_libkrun)]
const VIRGLRENDERER_NO_VIRGL: u32 = 1 << 7;
// GPU shared-memory (vRAM) window. 1 GiB matches krunkit's default.
#[cfg(have_libkrun)]
const GPU_SHM_BYTES: u64 = 1 << 30;

/// The OVMF/EDK2 firmware the EFI guest boots, embedded at build time so the
/// binary is self-contained (survives the self-sign re-exec). macOS only.
#[cfg(all(have_libkrun, target_os = "macos"))]
const FIRMWARE: &[u8] = include_bytes!(env!("KRUN_EFI_FIRMWARE"));

/// Parameters for booting a Linux guest under libkrun. The payload fields differ
/// by host (the boot models are fundamentally different); the rest is shared.
pub struct BootLinux {
    /// macOS host: a raw EFI-bootable disk image (e.g. a NixOS `raw-efi` image or
    /// a Fedora CoreOS raw). The guest's own bootloader/kernel live in it; the
    /// embedded OVMF boots it.
    #[cfg(target_os = "macos")]
    pub disk: PathBuf,
    /// Linux host: a rootfs directory shared into the guest over virtiofs as `/`.
    /// The bundled `libkrunfw` kernel boots it.
    #[cfg(target_os = "linux")]
    pub root: PathBuf,
    /// Linux host: the command (argv) run as the guest init, resolved inside the
    /// rootfs. `exec[0]` is the binary path (absolute, inside the rootfs).
    #[cfg(target_os = "linux")]
    pub exec: Vec<String>,
    /// Enable a virtio-gpu Venus device (GPU acceleration). On macOS this is the
    /// only way a Linux guest gets `/dev/dri` (via `MoltenVK`); on Linux it maps to
    /// the host DRM. Off by default.
    pub gpu: bool,
    pub cpus: u8,
    pub memory_mib: u32,
    /// Capture the guest serial console to this file instead of inheriting the
    /// process stdio. `krun_start_enter` takes over the process, so a file is how
    /// a background/lockstep caller reads the console after the VM stops.
    pub console_file: Option<PathBuf>,
    /// Stop the VM and exit after this long (a watchdog, so a background
    /// invocation never hangs).
    pub timeout: Duration,
}

#[cfg(have_libkrun)]
fn cstr(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error::Nul {
        path: s.to_owned(),
    })
}

/// Write the embedded firmware to a per-user cache file (once) and return its
/// path, since `krun_set_firmware` takes a path, not bytes. macOS only.
#[cfg(all(have_libkrun, target_os = "macos"))]
fn firmware_path() -> Result<PathBuf, Error> {
    use std::os::unix::fs::PermissionsExt;

    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .ok_or_else(|| Error::Setup {
            message: "no HOME or XDG_CACHE_HOME for firmware cache".to_owned(),
        })?
        .join("ix")
        .join("vmkit");
    std::fs::create_dir_all(&cache).map_err(|e| Error::Setup {
        message: format!("create firmware cache dir: {e}"),
    })?;
    // Owner-only, matching the signed-binary cache in `main.rs`.
    let _ = std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700));
    // Name by content length so a firmware change lands a fresh file; the blob is
    // immutable for a given build.
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
        std::fs::write(&tmp, FIRMWARE).map_err(|e| Error::Setup {
            message: format!("write firmware: {e}"),
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| Error::Setup {
            message: format!("publish firmware: {e}"),
        })?;
    }
    Ok(path)
}

#[cfg(have_libkrun)]
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

/// Set the macOS (libkrun-efi) boot payload: the embedded OVMF firmware plus the
/// guest's raw EFI disk. The returned `CString`s must outlive the krun calls, so
/// the caller keeps them alive until `krun_start_enter`.
#[cfg(all(have_libkrun, target_os = "macos"))]
fn set_payload(ctx: u32, boot: &BootLinux) -> Result<Vec<CString>, Error> {
    if !boot.disk.exists() {
        return Err(Error::Source {
            path: boot.disk.clone(),
            message: "disk image does not exist".to_owned(),
        });
    }
    let firmware = firmware_path()?;
    let disk = boot
        .disk
        .canonicalize()
        .unwrap_or_else(|_| boot.disk.clone());
    let firmware_c = cstr(&firmware.to_string_lossy())?;
    let disk_c = cstr(&disk.to_string_lossy())?;
    let block_id = cstr("root")?;
    // Safety: pointers are valid for the call; krun copies what it needs.
    unsafe {
        check("krun_set_firmware", krun_set_firmware(ctx, firmware_c.as_ptr()))?;
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
    }
    Ok(vec![firmware_c, disk_c, block_id])
}

/// Set the Linux (classic libkrun) boot payload: a rootfs directory plus the
/// exec command run as the guest init under the bundled kernel. Returns the
/// `CString`s (and the argv pointer-array backing store) to keep alive across the
/// krun calls.
#[cfg(all(have_libkrun, target_os = "linux"))]
fn set_payload(ctx: u32, boot: &BootLinux) -> Result<Vec<CString>, Error> {
    if !boot.root.is_dir() {
        return Err(Error::Source {
            path: boot.root.clone(),
            message: "rootfs path is not a directory".to_owned(),
        });
    }
    let root = boot
        .root
        .canonicalize()
        .unwrap_or_else(|_| boot.root.clone());
    let root_c = cstr(&root.to_string_lossy())?;
    let workdir_c = cstr("/")?;

    // exec[0] is the binary; libkrun uses `exec_path` as the guest argv[0], so the
    // `argv` it takes is only the *arguments* after the binary (see libkrun
    // examples/chroot_vm.c: `krun_set_exec(ctx, guest_argv[0], &guest_argv[1], ..)`).
    // Passing the full vec would duplicate argv[0] (e.g. `/bin/sh /bin/sh -c ...`,
    // making sh try to run the binary as a script).
    let default_exec = [String::from("/bin/sh")];
    let exec: &[String] = if boot.exec.is_empty() {
        &default_exec
    } else {
        &boot.exec
    };
    let exec_path_c = cstr(&exec[0])?;
    let argv_c: Vec<CString> = exec[1..]
        .iter()
        .map(|a| cstr(a))
        .collect::<Result<_, _>>()?;
    // NULL-terminated argv pointer array, kept alive alongside `argv_c`.
    let mut argv_ptrs: Vec<*const c_char> = argv_c.iter().map(|c| c.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());
    // Minimal env so the guest init resolves common tools without inheriting the
    // host environment. NULL-terminated.
    let env_c = vec![cstr("PATH=/run/current-system/sw/bin:/bin:/usr/bin")?];
    let mut env_ptrs: Vec<*const c_char> = env_c.iter().map(|c| c.as_ptr()).collect();
    env_ptrs.push(std::ptr::null());

    // Safety: pointers are valid for the call; krun copies what it needs. The
    // pointer arrays and their backing CStrings are returned to outlive the call.
    unsafe {
        check("krun_set_root", krun_set_root(ctx, root_c.as_ptr()))?;
        check("krun_set_workdir", krun_set_workdir(ctx, workdir_c.as_ptr()))?;
        check(
            "krun_set_exec",
            krun_set_exec(ctx, exec_path_c.as_ptr(), argv_ptrs.as_ptr(), env_ptrs.as_ptr()),
        )?;
    }
    // Keep every CString alive past the calls above (the pointer arrays borrow
    // them); they are dropped after `krun_start_enter` in the caller.
    let mut keep = vec![root_c, workdir_c, exec_path_c];
    keep.extend(argv_c);
    keep.extend(env_c);
    Ok(keep)
}

/// Stub for builds without the libkrun backend (e.g. a Linux->darwin cross
/// build, where libkrun-efi is unavailable). Returns a typed error rather than
/// silently doing nothing, so a caller learns the backend was not compiled in.
#[cfg(not(have_libkrun))]
pub fn boot_linux(_boot: &BootLinux) -> Result<(), Error> {
    Err(Error::NotBuilt)
}

/// Boot a Linux guest under libkrun and run it until it powers off or the timeout
/// elapses. Does not return on success: `krun_start_enter` takes over the process
/// and `exit()`s with the guest's exit code when the VM stops.
#[cfg(have_libkrun)]
pub fn boot_linux(boot: &BootLinux) -> Result<(), Error> {
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
        eprintln!("vmkit: timeout reached, stopping");
        std::process::exit(0);
    });

    // Safety: every pointer passed below outlives its call (the CStrings live to
    // the end of this function, and krun copies what it needs); krun_start_enter
    // consumes the context.
    unsafe {
        check("krun_set_log_level", krun_set_log_level(2))?; // warn
        let ctx = krun_create_ctx();
        // `krun_create_ctx` returns a non-negative ctx id or `-errno`. `try_from`
        // both narrows to the u32 the rest of the API takes and rejects an error
        // return, so no sign-losing `as` cast is needed.
        let ctx = u32::try_from(ctx).map_err(|_| Error::Libkrun {
            op: "krun_create_ctx".to_owned(),
            code: ctx,
        })?;
        check(
            "krun_set_vm_config",
            krun_set_vm_config(ctx, boot.cpus, boot.memory_mib),
        )?;
        // Host-specific payload (firmware+disk on macOS, rootfs+exec on Linux).
        // The returned CStrings stay alive until after krun_start_enter.
        let _payload = set_payload(ctx, boot)?;
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
        // guest exit code when the VM powers off. A negative return is a pre-boot
        // config error.
        let code = krun_start_enter(ctx);
        Err(Error::Libkrun {
            op: "krun_start_enter".to_owned(),
            code,
        })
    }
}
