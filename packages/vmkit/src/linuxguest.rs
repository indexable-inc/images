//! Linux GUI guest support: boot an aarch64 Linux guest with a virtio-gpu
//! display and USB keyboard/mouse fully off-screen, and screenshot its
//! framebuffer.
//!
//! This mirrors [`crate::macguest`] but swaps the Mac-only platform/graphics
//! classes for the generic/virtio ones: a `VZGenericPlatformConfiguration`
//! booted by `VZEFIBootLoader` off a raw EFI disk, a `VZVirtioGraphicsDevice`
//! scanout for the display, and USB HID. The off-screen view, `IOSurface`
//! capture, and synthetic input are all guest-agnostic, so they are reused from
//! `macguest` verbatim ([`crate::macguest::start_vm_offscreen`],
//! [`crate::macguest::schedule_captures`], [`crate::macguest::capture`]).
//!
//! VZ's virtio-gpu is a 2D scanout with no 3D acceleration, so a wgpu guest app
//! renders through Mesa's software Vulkan (lavapipe). The host reads the same
//! framebuffer `IOSurface` as for a macOS guest.

use std::path::{Path, PathBuf};

use objc2::AllocAnyThread;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSArray, NSFileHandle, NSPipe};
use objc2_virtualization::{
    VZBootLoader, VZDiskImageStorageDeviceAttachment, VZEFIBootLoader, VZEFIVariableStore,
    VZEFIVariableStoreInitializationOptions, VZEntropyDeviceConfiguration,
    VZFileHandleSerialPortAttachment, VZGenericPlatformConfiguration,
    VZGraphicsDeviceConfiguration, VZKeyboardConfiguration, VZMemoryBalloonDeviceConfiguration,
    VZPlatformConfiguration, VZPointingDeviceConfiguration, VZSerialPortAttachment,
    VZSerialPortConfiguration, VZStorageDeviceConfiguration, VZUSBKeyboardConfiguration,
    VZUSBScreenCoordinatePointingDeviceConfiguration, VZVirtioBlockDeviceConfiguration,
    VZVirtioConsoleDeviceSerialPortConfiguration, VZVirtioEntropyDeviceConfiguration,
    VZVirtioGraphicsDeviceConfiguration, VZVirtioGraphicsScanoutConfiguration,
    VZVirtioTraditionalMemoryBalloonDeviceConfiguration, VZVirtualMachineConfiguration,
};

use crate::imp::{Error, file_url, ns_error_message};

/// Display geometry for the guest's single virtio-gpu scanout.
const DISPLAY_WIDTH: isize = 1920;
const DISPLAY_HEIGHT: isize = 1080;

/// Parameters for booting a Linux GUI guest and screenshotting it. A named
/// struct (like [`crate::linuxkrun::BootLinux`]) so callers and the future IPC
/// layer name each field; every field is `Send`.
pub struct LinuxGuiBoot {
    /// Raw EFI-bootable disk image (e.g. a NixOS `raw-efi` image).
    pub disk: PathBuf,
    /// EFI variable store file. Opened if present, created otherwise, so boot
    /// entries written by the guest's bootloader persist across runs.
    pub var_store: PathBuf,
    /// Output path prefix for screenshots (`<out_prefix>.NNN.png`).
    pub out_prefix: PathBuf,
    /// Stop the VM and exit after this many seconds (final shot at the deadline).
    pub seconds: u64,
    /// Number of virtual CPUs.
    pub cpus: usize,
    /// Guest memory in MiB.
    pub memory_mib: u64,
}

/// Boot the Linux GUI guest off-screen and screenshot its framebuffer to
/// `<out_prefix>.NNN.png`. Nothing appears on the host desktop and the host
/// cursor is never touched.
pub fn boot_linux_gui(boot: LinuxGuiBoot) -> Result<(), Error> {
    let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;

    let config = build_linux_gui_config(&boot.disk, &boot.var_store, boot.cpus, boot.memory_mib)?;
    if let Err(error) = unsafe { config.validateWithError() } {
        return Err(Error::InvalidConfiguration {
            message: ns_error_message(&error),
        });
    }

    let vm_view = crate::macguest::start_vm_offscreen(mtm, &config);

    // Screenshot ticks: the hardcoded ones below the deadline, then the deadline
    // itself, so a short `--seconds` stops on time and always takes a final shot.
    let mut shots: Vec<u64> = [2, 18, 35, 55]
        .into_iter()
        .filter(|&t| t < boot.seconds)
        .collect();
    shots.push(boot.seconds);
    crate::macguest::schedule_captures(vm_view, boot.out_prefix, shots);

    // The view needs the `AppKit` run loop to build its layer tree and receive
    // guest frames; the capture thread exits the process when done.
    NSApplication::sharedApplication(mtm).run();
    Ok(())
}

/// Parameters for driving a Linux GUI guest interactively from stdin.
pub struct DriveLinux {
    /// Raw EFI-bootable disk image.
    pub disk: PathBuf,
    /// EFI variable store file (opened or created).
    pub var_store: PathBuf,
    /// Number of virtual CPUs.
    pub cpus: usize,
    /// Guest memory in MiB.
    pub memory_mib: u64,
}

/// Boot the Linux GUI guest off-screen and hand it to the shared stdin command
/// loop (synthetic keyboard/mouse + on-demand screenshots), so a caller can
/// drive and inspect it without touching the host cursor or desktop.
pub fn drive_linux(drive: DriveLinux) -> Result<(), Error> {
    let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;

    let config =
        build_linux_gui_config(&drive.disk, &drive.var_store, drive.cpus, drive.memory_mib)?;
    if let Err(error) = unsafe { config.validateWithError() } {
        return Err(Error::InvalidConfiguration {
            message: ns_error_message(&error),
        });
    }

    let view = crate::macguest::start_vm_offscreen(mtm, &config);
    crate::drive::drive_view(mtm, view, "Linux guest");
    Ok(())
}

/// Build the Linux GUI guest configuration: generic platform booted by EFI off
/// the raw disk, one virtio-gpu scanout, USB keyboard + pointing device, a
/// serial console to stdout (the guest must use `console=hvc0`), and entropy +
/// balloon. Shared by the screenshot ([`boot_linux_gui`]) and interactive
/// ([`drive_linux`]) paths.
fn build_linux_gui_config(
    disk: &Path,
    var_store_path: &Path,
    cpus: usize,
    memory_mib: u64,
) -> Result<Retained<VZVirtualMachineConfiguration>, Error> {
    let memory_bytes = memory_mib
        .checked_mul(1024 * 1024)
        .ok_or(Error::MemoryTooLarge { mib: memory_mib })?;

    let platform = unsafe { VZGenericPlatformConfiguration::new() };

    // EFI variable store: open an existing one, otherwise create it. Persisting
    // it keeps the guest bootloader's boot entries stable across runs.
    let var_url = file_url(var_store_path);
    let var_store = if var_store_path.exists() {
        unsafe { VZEFIVariableStore::initWithURL(VZEFIVariableStore::alloc(), &var_url) }
    } else {
        unsafe {
            VZEFIVariableStore::initCreatingVariableStoreAtURL_options_error(
                VZEFIVariableStore::alloc(),
                &var_url,
                VZEFIVariableStoreInitializationOptions(0),
            )
        }
        .map_err(|e| Error::InvalidConfiguration {
            message: ns_error_message(&e),
        })?
    };
    let boot_loader = unsafe { VZEFIBootLoader::new() };
    unsafe { boot_loader.setVariableStore(Some(&var_store)) };

    // virtio-gpu display: one scanout. 2D only (no 3D accel), so a wgpu guest
    // app renders via software Vulkan/lavapipe.
    let scanout = unsafe {
        VZVirtioGraphicsScanoutConfiguration::initWithWidthInPixels_heightInPixels(
            VZVirtioGraphicsScanoutConfiguration::alloc(),
            DISPLAY_WIDTH,
            DISPLAY_HEIGHT,
        )
    };
    let gfx = unsafe { VZVirtioGraphicsDeviceConfiguration::new() };
    unsafe { gfx.setScanouts(&NSArray::from_slice(&[&*scanout])) };

    // Root disk (the NixOS aarch64 EFI image).
    let disk_url = file_url(disk);
    let disk_attach = unsafe {
        VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &disk_url,
            false,
        )
    }
    .map_err(|e| Error::Bundle {
        message: ns_error_message(&e),
    })?;
    let block = unsafe {
        VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &disk_attach,
        )
    };

    let keyboard = unsafe { VZUSBKeyboardConfiguration::new() };
    let pointing = unsafe { VZUSBScreenCoordinatePointingDeviceConfiguration::new() };

    // Guest serial console -> our stdout for boot debugging. VZ rejects a null
    // read handle, so give it the (unwritten) read end of a fresh pipe.
    let pipe = NSPipe::pipe();
    let read_handle = pipe.fileHandleForReading();
    let stdout_handle = NSFileHandle::fileHandleWithStandardOutput();
    let serial_attachment = unsafe {
        VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
            VZFileHandleSerialPortAttachment::alloc(),
            Some(&read_handle),
            Some(&stdout_handle),
        )
    };
    let serial = unsafe { VZVirtioConsoleDeviceSerialPortConfiguration::new() };
    let serial_attachment_ref: &VZSerialPortAttachment = &serial_attachment;
    unsafe { serial.setAttachment(Some(serial_attachment_ref)) };

    let entropy = unsafe { VZVirtioEntropyDeviceConfiguration::new() };
    let balloon = unsafe { VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new() };

    let config = unsafe { VZVirtualMachineConfiguration::new() };
    let platform_ref: &VZPlatformConfiguration = &platform;
    let boot_ref: &VZBootLoader = &boot_loader;
    let gfx_ref: &VZGraphicsDeviceConfiguration = &gfx;
    let block_ref: &VZStorageDeviceConfiguration = &block;
    let kbd_ref: &VZKeyboardConfiguration = &keyboard;
    let pt_ref: &VZPointingDeviceConfiguration = &pointing;
    let serial_ref: &VZSerialPortConfiguration = &serial;
    let entropy_ref: &VZEntropyDeviceConfiguration = &entropy;
    let balloon_ref: &VZMemoryBalloonDeviceConfiguration = &balloon;
    unsafe {
        config.setPlatform(platform_ref);
        config.setBootLoader(Some(boot_ref));
        config.setCPUCount(cpus);
        config.setMemorySize(memory_bytes);
        config.setGraphicsDevices(&NSArray::from_slice(&[gfx_ref]));
        config.setStorageDevices(&NSArray::from_slice(&[block_ref]));
        config.setKeyboards(&NSArray::from_slice(&[kbd_ref]));
        config.setPointingDevices(&NSArray::from_slice(&[pt_ref]));
        config.setSerialPorts(&NSArray::from_slice(&[serial_ref]));
        config.setEntropyDevices(&NSArray::from_slice(&[entropy_ref]));
        config.setMemoryBalloonDevices(&NSArray::from_slice(&[balloon_ref]));
    }

    Ok(config)
}
