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
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSData, NSError, NSPoint, NSRect, NSSize};
use objc2_io_surface::{IOSurfaceLockOptions, IOSurfaceRef};
// Named explicitly so the dependency is a direct, visible use (the type is
// otherwise only reachable through `NSView::layer()`'s return type).
use objc2_quartz_core::CALayer;

/// kIOSurfaceLockReadOnly: read-only lock, no dirty tracking.
const LOCK_READ_ONLY: IOSurfaceLockOptions = IOSurfaceLockOptions(1);
use objc2_virtualization::{
    VZBootLoader, VZDiskImageStorageDeviceAttachment, VZGraphicsDeviceConfiguration,
    VZKeyboardConfiguration, VZMacAuxiliaryStorage, VZMacGraphicsDeviceConfiguration,
    VZMacGraphicsDisplayConfiguration, VZMacHardwareModel, VZMacMachineIdentifier,
    VZMacOSBootLoader, VZMacPlatformConfiguration, VZNATNetworkDeviceAttachment,
    VZNetworkDeviceConfiguration, VZPlatformConfiguration, VZPointingDeviceConfiguration,
    VZStorageDeviceConfiguration, VZUSBKeyboardConfiguration,
    VZUSBScreenCoordinatePointingDeviceConfiguration, VZVirtioBlockDeviceConfiguration,
    VZVirtioNetworkDeviceConfiguration, VZVirtualMachine, VZVirtualMachineConfiguration,
    VZVirtualMachineView,
};

use crate::imp::{Error, file_url, ns_error_message};

/// Parameters for booting a macOS guest and screenshotting it.
pub struct MacBootScreenshot {
    pub bundle: PathBuf,
    pub out_prefix: PathBuf,
    pub seconds: f64,
}

pub fn boot_macos_screenshot(boot: MacBootScreenshot) -> Result<(), Error> {
    let mtm = MainThreadMarker::new().ok_or(Error::NotMainThread)?;

    let config = build_macos_config(&boot.bundle)?;
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
    // start directly. dispatch_main below drains the queue so the completion
    // handler fires. `vm` and `completion` live for the process because this
    // function never returns (dispatch_main diverges), and `vm_view` retains the
    // VM as well.
    unsafe { vm.startWithCompletionHandler(&completion) };

    // Capture loop on the main queue (AppKit objects must be touched there).
    let seconds = boot.seconds;
    let out_prefix = boot.out_prefix.clone();
    let shots: Vec<f64> = vec![2.0, 18.0, 35.0, 55.0, seconds];
    let view_for_caps = vm_view;
    schedule_captures(view_for_caps, out_prefix, shots, seconds);

    // VZVirtualMachineView needs the AppKit run loop to build its layer tree and
    // receive guest frames; the capture thread exits the process when done.
    app.run();
    Ok(())
}

/// Schedule screenshots on the main queue at increasing delays, then exit.
fn schedule_captures(
    view: Retained<VZVirtualMachineView>,
    out_prefix: PathBuf,
    shots: Vec<f64>,
    deadline: f64,
) {
    // A background thread sleeps between shots and hops each capture onto the
    // main queue (AppKit/IOSurface access must be on the main thread). The view
    // is not Send, so we move only the raw pointer and re-borrow on the main
    // queue, where it is valid.
    let view_ptr = Retained::into_raw(view) as usize;
    std::thread::spawn(move || {
        let mut elapsed = 0.0;
        for t in shots {
            if t > elapsed {
                std::thread::sleep(Duration::from_secs_f64(t - elapsed));
                elapsed = t;
            }
            let path = out_prefix.with_extension(format!("{:03}.png", t as u64));
            let p = path.clone();
            DispatchQueue::main().exec_sync(move || {
                // Safety: the view lives for the process lifetime (leaked above)
                // and we only touch it on the main queue.
                let view: &VZVirtualMachineView = unsafe { &*(view_ptr as *const VZVirtualMachineView) };
                match capture(view, &p) {
                    Ok(bytes) => eprintln!("macos-vm: wrote {bytes} bytes -> {}", p.display()),
                    Err(error) => eprintln!("macos-vm: capture: {error}"),
                }
            });
            if elapsed >= deadline {
                break;
            }
        }
        eprintln!("macos-vm: done");
        std::process::exit(0);
    });
}

/// The guest framebuffer IOSurface object, if the view has started rendering.
fn frame_surface(view: &VZVirtualMachineView) -> Option<Retained<AnyObject>> {
    let first = view.subviews().firstObject()?;
    let layer: Retained<CALayer> = first.layer()?;
    unsafe { layer.contents() }
}

/// Read the IOSurface bytes (BGRA) and encode a PNG.
fn capture(view: &VZVirtualMachineView, path: &Path) -> Result<usize, Error> {
    let contents = frame_surface(view).ok_or(Error::NoFramebuffer)?;
    // The layer contents is an IOSurface, toll-free bridged to IOSurfaceRef.
    let surface: &IOSurfaceRef = unsafe { &*Retained::as_ptr(&contents).cast::<IOSurfaceRef>() };

    let (width, height, rgba) = unsafe {
        let _ = surface.lock(LOCK_READ_ONLY, std::ptr::null_mut());
        let width = surface.width();
        let height = surface.height();
        let bytes_per_row = surface.bytes_per_row();
        let base = surface.base_address().as_ptr() as *const u8;

        let mut rgba = vec![0u8; width * height * 4];
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
        let _ = surface.unlock(LOCK_READ_ONLY, std::ptr::null_mut());
        (width, height, rgba)
    };

    let mut buf = std::io::Cursor::new(Vec::new());
    image::ImageEncoder::write_image(
        image::codecs::png::PngEncoder::new(&mut buf),
        &rgba,
        width as u32,
        height as u32,
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| Error::CaptureEncode { message: e.to_string() })?;
    let png = buf.into_inner();
    std::fs::write(path, &png).map_err(|e| Error::CaptureEncode { message: e.to_string() })?;
    Ok(png.len())
}

fn build_macos_config(bundle: &Path) -> Result<Retained<VZVirtualMachineConfiguration>, Error> {
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
    .ok_or(Error::Bundle { message: "invalid hardware model".into() })?;
    let machine_id = unsafe {
        VZMacMachineIdentifier::initWithDataRepresentation(
            VZMacMachineIdentifier::alloc(),
            &NSData::with_bytes(&id_data),
        )
    }
    .ok_or(Error::Bundle { message: "invalid machine id".into() })?;

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
    Ok(config)
}
