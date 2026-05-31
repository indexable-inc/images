//! macOS guest support: boot an installed macOS guest fully off-screen and
//! screenshot its display with no window and no Screen-Recording permission.
//!
//! The guest framebuffer is an `IOSurface` living in the
//! `VZVirtualMachineView`'s framebuffer subview's layer contents. We read its
//! BGRA bytes directly and encode PNG with the pure-Rust `image` crate, entirely
//! in-process. The view lives in an off-screen, non-activating window, so the
//! host desktop and cursor are never touched. Technique from
//! github.com/thecrypticace/vzautomation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, dispatch_main};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSData, NSError, NSPoint, NSRect, NSSize, NSString};
use objc2_io_surface::{IOSurface, IOSurfaceLockOptions, IOSurfaceRef};
// Named explicitly so the dependency is a direct, visible use (the type is
// otherwise only reachable through `NSView::layer()`'s return type).
use objc2_quartz_core::CALayer;
use objc2_virtualization::{
    VZBootLoader, VZDirectoryShare, VZDirectorySharingDeviceConfiguration,
    VZDiskImageStorageDeviceAttachment, VZGraphicsDeviceConfiguration, VZKeyboardConfiguration,
    VZMacAuxiliaryStorage, VZMacGraphicsDeviceConfiguration, VZMacGraphicsDisplayConfiguration,
    VZMacHardwareModel, VZMacMachineIdentifier, VZMacAuxiliaryStorageInitializationOptions,
    VZMacOSBootLoader, VZMacOSInstaller, VZMacOSRestoreImage, VZMacPlatformConfiguration,
    VZNATNetworkDeviceAttachment, VZNetworkDeviceConfiguration, VZPlatformConfiguration,
    VZPointingDeviceConfiguration, VZSharedDirectory, VZSingleDirectoryShare,
    VZStorageDeviceConfiguration, VZUSBKeyboardConfiguration,
    VZUSBScreenCoordinatePointingDeviceConfiguration, VZVirtioBlockDeviceConfiguration,
    VZVirtioFileSystemDeviceConfiguration, VZVirtioNetworkDeviceConfiguration, VZVirtualMachine,
    VZVirtualMachineConfiguration, VZVirtualMachineView,
};

use crate::imp::{Error, file_url, ns_error_message};

/// `kCVPixelFormatType_32BGRA` ('BGRA'): the layout the `IOSurface` read assumes.
const PIXEL_FORMAT_BGRA: u32 = 0x4247_5241;

/// A host directory shared into the guest over virtio-fs (read-write).
pub struct DirShare {
    pub tag: ShareTag,
    pub host_dir: PathBuf,
}

/// The virtio-fs mount tag. `Automount` uses the special macOS tag that mounts
/// the share automatically at `/Volumes/My Shared Files` with no guest-side
/// `mount`; `Named` requires the guest to mount the tag explicitly.
pub enum ShareTag {
    Automount,
    Named(String),
}

/// Parameters for booting a macOS guest and screenshotting it.
pub struct MacBootScreenshot {
    pub bundle: PathBuf,
    pub out_prefix: PathBuf,
    pub seconds: u64,
    pub shares: Vec<DirShare>,
}

/// Build the off-screen window + `VZVirtualMachineView`, create the VM, and
/// start it. Returns the view (retained; the caller leaks it for the process
/// lifetime and drives the `AppKit` run loop). Shared by the screenshot path and
/// the interactive driver, which differ only in what they do with the run loop.
pub fn start_guest_offscreen(
    mtm: MainThreadMarker,
    bundle: &Path,
    shares: &[DirShare],
) -> Result<Retained<VZVirtualMachineView>, Error> {
    let config = build_macos_config(bundle, shares)?;
    if let Err(error) = unsafe { config.validateWithError() } {
        return Err(Error::InvalidConfiguration {
            message: ns_error_message(&error),
        });
    }

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);

    let vm = unsafe { VZVirtualMachine::initWithConfiguration(VZVirtualMachine::alloc(), &config) };

    // Off-screen, borderless window. Never visible; the host cursor is never
    // captured. We read the guest IOSurface, not the on-screen composite, so an
    // off-screen window is fine.
    let frame = NSRect::new(NSPoint::new(-20000.0, -20000.0), NSSize::new(1920.0, 1080.0));
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    let view_frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1920.0, 1080.0));
    let vm_view = unsafe { VZVirtualMachineView::initWithFrame(mtm.alloc(), view_frame) };
    unsafe { vm_view.setVirtualMachine(Some(&vm)) };
    window.setContentView(Some(&vm_view));
    window.orderFrontRegardless();

    let completion = RcBlock::new(|error: *mut NSError| {
        if error.is_null() {
            eprintln!("macos-vm: guest started");
        } else {
            let error = unsafe { &*error };
            eprintln!("macos-vm: guest failed to start: {}", ns_error_message(error));
            std::process::exit(1);
        }
    });
    // We hold a MainThreadMarker, so we are on the main thread (the VM's queue);
    // start directly. The caller's `app.run()` drives the main run loop, which
    // services the main dispatch queue so the completion handler fires. The
    // window retains the view, which retains the VM; the caller keeps the view
    // (and thus the VM) alive for the process by leaking it.
    unsafe { vm.startWithCompletionHandler(&completion) };
    std::mem::forget(window);
    std::mem::forget(completion);
    Ok(vm_view)
}

pub fn boot_macos_screenshot(boot: MacBootScreenshot) -> Result<(), Error> {
    let MacBootScreenshot {
        bundle,
        out_prefix,
        seconds,
        shares,
    } = boot;

    let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;
    let vm_view = start_guest_offscreen(mtm, &bundle, &shares)?;

    // Tick times for screenshots: the hardcoded ticks below the deadline, then
    // the deadline itself, so a short `--seconds` stops on time and always takes
    // a final shot.
    let mut shots: Vec<u64> = [2, 18, 35, 55].into_iter().filter(|&t| t < seconds).collect();
    shots.push(seconds);
    schedule_captures(vm_view, out_prefix, shots);

    // VZVirtualMachineView needs the `AppKit` run loop to build its layer tree and
    // receive guest frames; the capture thread exits the process when done.
    NSApplication::sharedApplication(mtm).run();
    Ok(())
}

/// Sleep between ticks and hop each capture onto the main queue (`AppKit` and
/// `IOSurface` access must be on the main thread), then exit the process.
fn schedule_captures(view: Retained<VZVirtualMachineView>, out_prefix: PathBuf, shots: Vec<u64>) {
    // The view is not `Send`, so move only the raw pointer and re-borrow on the
    // main queue, where it is valid. The view is leaked (lives for the process)
    // and also retained by the window, so the reborrow is never a use-after-free.
    let view_ptr = Retained::into_raw(view) as usize;
    std::thread::spawn(move || {
        let mut elapsed = 0u64;
        for t in shots {
            if t > elapsed {
                std::thread::sleep(Duration::from_secs(t - elapsed));
                elapsed = t;
            }
            let path = out_prefix.with_extension(format!("{t:03}.png"));
            let p = path.clone();
            DispatchQueue::main().exec_sync(move || {
                // Safety: the view lives for the process (leaked above) and we
                // only touch it on the main queue.
                let view: &VZVirtualMachineView =
                    unsafe { &*(view_ptr as *const VZVirtualMachineView) };
                match capture(view, &p) {
                    Ok(bytes) => eprintln!("macos-vm: wrote {bytes} bytes -> {}", p.display()),
                    Err(error) => eprintln!("macos-vm: capture: {error}"),
                }
            });
        }
        eprintln!("macos-vm: done");
        std::process::exit(0);
    });
}

/// The guest framebuffer object (an `IOSurface`), if the view has started
/// rendering. Returns the raw layer contents; the caller verifies the type.
fn frame_contents(view: &VZVirtualMachineView) -> Option<Retained<AnyObject>> {
    let first = view.subviews().firstObject()?;
    let layer: Retained<CALayer> = first.layer()?;
    unsafe { layer.contents() }
}

/// Read the framebuffer `IOSurface` (BGRA) and encode a PNG.
pub fn capture(view: &VZVirtualMachineView, path: &Path) -> Result<usize, Error> {
    let contents = frame_contents(view).ok_or(Error::NoFramebuffer)?;
    // Verify the layer contents really is an IOSurface before any unsafe access.
    let surface_obj: Retained<IOSurface> =
        contents.downcast::<IOSurface>().map_err(|_| Error::NoFramebuffer)?;
    // `IOSurface` (objc) is toll-free bridged to `IOSurfaceRef` (CF), which
    // carries the data accessors.
    let surface: &IOSurfaceRef =
        unsafe { &*Retained::as_ptr(&surface_obj).cast::<IOSurfaceRef>() };

    let width = surface.width();
    let height = surface.height();
    // Only the single-plane 32-bit BGRA layout is handled; reject anything else
    // rather than read past the mapping or produce garbage.
    if surface.plane_count() > 1
        || surface.bytes_per_element() != 4
        || surface.pixel_format() != PIXEL_FORMAT_BGRA
    {
        return Err(Error::CaptureEncode {
            message: format!(
                "unexpected IOSurface layout: planes={} bpe={} format={:#x}",
                surface.plane_count(),
                surface.bytes_per_element(),
                surface.pixel_format()
            ),
        });
    }

    // Allocate before locking so an allocation failure cannot leak the lock; the
    // locked region below does only an in-bounds memcpy (no panics, no `?`).
    let mut rgba = vec![0u8; width * height * 4];
    unsafe {
        let _ = surface.lock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut());
        let bytes_per_row = surface.bytes_per_row();
        let base = surface.base_address().as_ptr() as *const u8;
        for y in 0..height {
            let row = base.add(y * bytes_per_row);
            for x in 0..width {
                let p = row.add(x * 4);
                let o = (y * width + x) * 4;
                rgba[o] = *p.add(2); // R <- BGRA.R
                rgba[o + 1] = *p.add(1); // G
                rgba[o + 2] = *p; // B
                rgba[o + 3] = *p.add(3); // A
            }
        }
        let _ = surface.unlock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut());
    }

    let w = u32::try_from(width).map_err(|_| Error::CaptureEncode { message: "width too large".into() })?;
    let h = u32::try_from(height).map_err(|_| Error::CaptureEncode { message: "height too large".into() })?;
    let mut buf = std::io::Cursor::new(Vec::new());
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut buf),
        &rgba,
        w,
        h,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| Error::CaptureEncode { message: e.to_string() })?;
    let png = buf.into_inner();
    std::fs::write(path, &png).map_err(|e| Error::CaptureEncode { message: e.to_string() })?;
    Ok(png.len())
}

/// Install macOS into a fresh bundle directory from a local restore image
/// (IPSW). The online catalog (gdmf) is bypassed by taking a local file, since
/// gdmf is TLS-intercepted on some networks. Writes disk/aux/hardware-model/
/// machine-id so [`boot_macos_screenshot`] can later boot the bundle.
pub fn install_macos(ipsw: PathBuf, bundle: PathBuf, disk_gib: u64) -> Result<(), Error> {
    std::fs::create_dir_all(&bundle).map_err(|e| Error::Bundle { message: e.to_string() })?;
    let ipsw_url = file_url(&ipsw);

    eprintln!("macos-vm: loading restore image {} ...", ipsw.display());
    // load completion fires on an arbitrary thread; extract the (Send) hardware
    // model bytes there, then build the VM + installer on the main queue.
    let load_done = RcBlock::new(move |image: *mut VZMacOSRestoreImage, error: *mut NSError| {
        if !error.is_null() {
            eprintln!("macos-vm: load restore image: {}", ns_error_message(unsafe { &*error }));
            std::process::exit(1);
        }
        let image = unsafe { &*image };
        let Some(req) = (unsafe { image.mostFeaturefulSupportedConfiguration() }) else {
            eprintln!("macos-vm: restore image has no configuration supported by this host");
            std::process::exit(1);
        };
        let hw = unsafe { req.hardwareModel() };
        if !unsafe { hw.isSupported() } {
            eprintln!("macos-vm: hardware model not supported by this host");
            std::process::exit(1);
        }
        let hw_data = unsafe { hw.dataRepresentation() }.to_vec();
        let bundle = bundle.clone();
        let ipsw = ipsw.clone();
        DispatchQueue::main().exec_async(move || {
            if let Err(error) = start_install(&bundle, &ipsw, &hw_data, disk_gib) {
                eprintln!("macos-vm: {error}");
                std::process::exit(1);
            }
        });
    });
    unsafe { VZMacOSRestoreImage::loadFileURL_completionHandler(&ipsw_url, &load_done) };

    dispatch_main();
}

/// On the main queue: materialize the bundle, then run `VZMacOSInstaller`.
fn start_install(bundle: &Path, ipsw: &Path, hw_data: &[u8], disk_gib: u64) -> Result<(), Error> {
    std::fs::write(bundle.join("hardware-model.bin"), hw_data)
        .map_err(|e| Error::Bundle { message: format!("write hardware-model.bin: {e}") })?;

    let machine_id = unsafe { VZMacMachineIdentifier::new() };
    let id_data = unsafe { machine_id.dataRepresentation() }.to_vec();
    std::fs::write(bundle.join("machine-id.bin"), &id_data)
        .map_err(|e| Error::Bundle { message: format!("write machine-id.bin: {e}") })?;

    if disk_gib == 0 {
        return Err(Error::Bundle { message: "disk-gib must be greater than 0".into() });
    }
    let disk_bytes = disk_gib
        .checked_mul(1024 * 1024 * 1024)
        .ok_or_else(|| Error::Bundle { message: format!("disk-gib {disk_gib} is too large") })?;
    let disk = bundle.join("disk.img");
    let file = std::fs::File::create(&disk).map_err(|e| Error::Bundle { message: format!("create disk.img: {e}") })?;
    file.set_len(disk_bytes)
        .map_err(|e| Error::Bundle { message: format!("size disk.img: {e}") })?;

    // Auxiliary storage: create fresh (remove any stale copy so the no-overwrite
    // initializer succeeds on re-install).
    let aux_path = bundle.join("aux.img");
    let _ = std::fs::remove_file(&aux_path);
    let hw = unsafe {
        VZMacHardwareModel::initWithDataRepresentation(
            VZMacHardwareModel::alloc(),
            &NSData::with_bytes(hw_data),
        )
    }
    .ok_or_else(|| Error::Bundle { message: "invalid hardware model".into() })?;
    let aux_url = file_url(&aux_path);
    unsafe {
        VZMacAuxiliaryStorage::initCreatingStorageAtURL_hardwareModel_options_error(
            VZMacAuxiliaryStorage::alloc(),
            &aux_url,
            &hw,
            VZMacAuxiliaryStorageInitializationOptions(0),
        )
    }
    .map_err(|e| Error::Bundle { message: format!("create aux storage: {}", ns_error_message(&e)) })?;

    let config = build_macos_config(bundle, &[])?;
    if let Err(error) = unsafe { config.validateWithError() } {
        return Err(Error::InvalidConfiguration {
            message: ns_error_message(&error),
        });
    }

    let vm = unsafe { VZVirtualMachine::initWithConfiguration(VZVirtualMachine::alloc(), &config) };
    let installer = unsafe {
        VZMacOSInstaller::initWithVirtualMachine_restoreImageURL(
            VZMacOSInstaller::alloc(),
            &vm,
            &file_url(ipsw),
        )
    };
    eprintln!("macos-vm: installing macOS into {} (this takes ~15-20 min) ...", bundle.display());

    let done = RcBlock::new(|error: *mut NSError| {
        if error.is_null() {
            println!("macos-vm: install complete");
            std::process::exit(0);
        }
        eprintln!("macos-vm: install failed: {}", ns_error_message(unsafe { &*error }));
        std::process::exit(1);
    });
    unsafe { installer.installWithCompletionHandler(&done) };
    // Keep the VM and installer alive for the duration of the install; the
    // process runs until the completion handler exits it.
    std::mem::forget(vm);
    std::mem::forget(installer);
    Ok(())
}

fn build_macos_config(
    bundle: &Path,
    shares: &[DirShare],
) -> Result<Retained<VZVirtualMachineConfiguration>, Error> {
    let hw_data = std::fs::read(bundle.join("hardware-model.bin"))
        .map_err(|e| Error::Bundle { message: format!("hardware-model.bin: {e}") })?;
    let id_data = std::fs::read(bundle.join("machine-id.bin"))
        .map_err(|e| Error::Bundle { message: format!("machine-id.bin: {e}") })?;

    let hw = unsafe {
        VZMacHardwareModel::initWithDataRepresentation(
            VZMacHardwareModel::alloc(),
            &NSData::with_bytes(&hw_data),
        )
    }
    .ok_or_else(|| Error::Bundle { message: "invalid hardware model".into() })?;
    let machine_id = unsafe {
        VZMacMachineIdentifier::initWithDataRepresentation(
            VZMacMachineIdentifier::alloc(),
            &NSData::with_bytes(&id_data),
        )
    }
    .ok_or_else(|| Error::Bundle { message: "invalid machine id".into() })?;

    let aux_url = file_url(&bundle.join("aux.img"));
    let aux = unsafe { VZMacAuxiliaryStorage::initWithURL(VZMacAuxiliaryStorage::alloc(), &aux_url) };

    let platform = unsafe { VZMacPlatformConfiguration::new() };
    unsafe {
        platform.setHardwareModel(&hw);
        platform.setMachineIdentifier(&machine_id);
        platform.setAuxiliaryStorage(Some(&aux));
    }

    let boot_loader = unsafe { VZMacOSBootLoader::new() };

    let display = unsafe {
        VZMacGraphicsDisplayConfiguration::initWithWidthInPixels_heightInPixels_pixelsPerInch(
            VZMacGraphicsDisplayConfiguration::alloc(),
            1920,
            1080,
            144,
        )
    };
    let gfx = unsafe { VZMacGraphicsDeviceConfiguration::new() };
    unsafe { gfx.setDisplays(&NSArray::from_slice(&[&*display])) };

    let disk_url = file_url(&bundle.join("disk.img"));
    let disk_attach = unsafe {
        VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
            VZDiskImageStorageDeviceAttachment::alloc(),
            &disk_url,
            false,
        )
    }
    .map_err(|e| Error::Bundle { message: ns_error_message(&e) })?;
    let block = unsafe {
        VZVirtioBlockDeviceConfiguration::initWithAttachment(
            VZVirtioBlockDeviceConfiguration::alloc(),
            &disk_attach,
        )
    };

    let net = unsafe { VZVirtioNetworkDeviceConfiguration::new() };
    let nat = unsafe { VZNATNetworkDeviceAttachment::new() };
    unsafe { net.setAttachment(Some(&nat)) };

    let keyboard = unsafe { VZUSBKeyboardConfiguration::new() };
    let pointing = unsafe { VZUSBScreenCoordinatePointingDeviceConfiguration::new() };

    // virtio-fs directory shares (held in a Vec so the retained devices outlive
    // the upcast refs handed to `setDirectorySharingDevices`).
    let fs_devices = shares
        .iter()
        .map(build_fs_device)
        .collect::<Result<Vec<_>, Error>>()?;

    let config = unsafe { VZVirtualMachineConfiguration::new() };
    let platform_ref: &VZPlatformConfiguration = &platform;
    let boot_ref: &VZBootLoader = &boot_loader;
    let gfx_ref: &VZGraphicsDeviceConfiguration = &gfx;
    let block_ref: &VZStorageDeviceConfiguration = &block;
    let net_ref: &VZNetworkDeviceConfiguration = &net;
    let kbd_ref: &VZKeyboardConfiguration = &keyboard;
    let pt_ref: &VZPointingDeviceConfiguration = &pointing;
    unsafe {
        config.setPlatform(platform_ref);
        config.setBootLoader(Some(boot_ref));
        config.setCPUCount(4);
        config.setMemorySize(8 * 1024 * 1024 * 1024);
        config.setGraphicsDevices(&NSArray::from_slice(&[gfx_ref]));
        config.setStorageDevices(&NSArray::from_slice(&[block_ref]));
        config.setNetworkDevices(&NSArray::from_slice(&[net_ref]));
        config.setKeyboards(&NSArray::from_slice(&[kbd_ref]));
        config.setPointingDevices(&NSArray::from_slice(&[pt_ref]));
    }
    if !fs_devices.is_empty() {
        let dev_refs: Vec<&VZDirectorySharingDeviceConfiguration> = fs_devices
            .iter()
            .map(|device| {
                let device_ref: &VZDirectorySharingDeviceConfiguration = device;
                device_ref
            })
            .collect();
        unsafe { config.setDirectorySharingDevices(&NSArray::from_slice(&dev_refs)) };
    }
    Ok(config)
}

/// Build one virtio-fs device for a shared host directory.
fn build_fs_device(
    share: &DirShare,
) -> Result<Retained<VZVirtioFileSystemDeviceConfiguration>, Error> {
    let tag = match &share.tag {
        ShareTag::Automount => unsafe {
            VZVirtioFileSystemDeviceConfiguration::macOSGuestAutomountTag()
        },
        ShareTag::Named(name) => {
            let tag = NSString::from_str(name);
            unsafe { VZVirtioFileSystemDeviceConfiguration::validateTag_error(&tag) }.map_err(
                |error| Error::Bundle {
                    message: format!("invalid share tag {name:?}: {}", ns_error_message(&error)),
                },
            )?;
            tag
        }
    };
    let dir_url = file_url(&share.host_dir);
    // Read-write: read-only sharing, if needed, belongs behind a dedicated flag
    // (see `parse_shares`), not an ambiguous in-path suffix.
    let shared = unsafe {
        VZSharedDirectory::initWithURL_readOnly(VZSharedDirectory::alloc(), &dir_url, false)
    };
    let single =
        unsafe { VZSingleDirectoryShare::initWithDirectory(VZSingleDirectoryShare::alloc(), &shared) };
    let device = unsafe {
        VZVirtioFileSystemDeviceConfiguration::initWithTag(
            VZVirtioFileSystemDeviceConfiguration::alloc(),
            &tag,
        )
    };
    let share_ref: &VZDirectoryShare = &single;
    unsafe { device.setShare(Some(share_ref)) };
    Ok(device)
}
