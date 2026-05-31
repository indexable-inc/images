//! `macos-vm`: drive Apple's Virtualization.framework from Rust.
//!
//! This binary owns a VM's lifecycle so that callers that cannot hold the
//! `com.apple.security.virtualization` entitlement themselves (notably the
//! ix-mcp Python interpreter, an unsigned immutable Nix store binary) can spawn
//! it and control a VM over IPC. The entitlement lives on *this* signed
//! process, never on the interpreter.
//!
//! v1 surface. `macos-vm info` reports whether virtualization is available.
//! `macos-vm boot-linux` boots a Linux guest from a raw kernel `Image` and
//! initramfs, streaming the guest serial console to stdout. boot-linux is the
//! end-to-end smoke path: a real guest reaching userspace proves the binding,
//! the entitlement, and the boot all work.
//!
//! The graphics/screenshot, vsock IPC, OCI-disk, and macOS-guest paths build on
//! the same `VZVirtualMachineConfiguration` and are tracked in the README.
//!
//! Off macOS the binary builds (so the Linux CI workspace graph stays green) but
//! is a single typed refusal: all Virtualization.framework code lives in the
//! `cfg(target_os = "macos")` module below, so the Linux compile sees only the
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
    /// Boot a Linux guest from a raw arm64 kernel `Image` + initramfs and
    /// stream its serial console to stdout until the guest stops or the timeout
    /// elapses.
    BootLinux {
        /// Path to an uncompressed Linux kernel image (arm64 raw `Image`, not a
        /// gzip/zboot `vmlinuz`).
        #[arg(long)]
        kernel: std::path::PathBuf,
        /// Path to an initramfs/initrd.
        #[arg(long)]
        initramfs: std::path::PathBuf,
        /// Number of virtual CPUs.
        #[arg(long, default_value_t = 2)]
        cpus: usize,
        /// Guest memory in MiB.
        #[arg(long, default_value_t = 1024)]
        memory_mib: u64,
        /// Kernel command line. `console=hvc0` routes the kernel console to the
        /// virtio console VZ exposes.
        #[arg(long, default_value = "console=hvc0")]
        cmdline: String,
        /// Stop the VM and exit after this many seconds.
        #[arg(long, default_value_t = 20)]
        timeout_secs: u64,
    },
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    let cli = Cli::parse();
    match imp::dispatch(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("macos-vm: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    eprintln!("macos-vm: requires macOS and Apple's Virtualization.framework");
    ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
mod imp {
    //! The Virtualization.framework glue.
    //!
    //! VZ binds a VM to a dispatch queue and requires every VM operation
    //! (`initWithConfiguration`, `start`, the completion handlers) to run on
    //! that queue. We use the main queue: the VM is created and started inside a
    //! block submitted to the main queue, and `dispatch_main` then drains that
    //! queue (mirroring Apple's sample app). objc2 objects are not `Send`, so
    //! the VM and its config must be built *inside* that block rather than moved
    //! into it; the block captures only the `Send` boot parameters.

    use std::path::PathBuf;
    use std::time::Duration;

    use block2::RcBlock;
    use dispatch2::{DispatchQueue, dispatch_main};
    use objc2::AllocAnyThread;
    use objc2::rc::Retained;
    use objc2_foundation::{NSArray, NSError, NSFileHandle, NSPipe, NSString, NSURL};
    use objc2_virtualization::{
        VZBootLoader, VZEntropyDeviceConfiguration, VZFileHandleSerialPortAttachment,
        VZLinuxBootLoader, VZMemoryBalloonDeviceConfiguration, VZSerialPortAttachment,
        VZSerialPortConfiguration, VZVirtioConsoleDeviceSerialPortConfiguration,
        VZVirtioEntropyDeviceConfiguration, VZVirtioTraditionalMemoryBalloonDeviceConfiguration,
        VZVirtualMachine, VZVirtualMachineConfiguration,
    };
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
    }

    /// Parameters for a Linux guest boot. A named struct rather than a wide
    /// tuple so callers (and the future IPC layer) name each field. Every field
    /// is `Send`, which is what lets the boot block be submitted to the queue.
    pub struct LinuxBoot {
        pub kernel: PathBuf,
        pub initramfs: PathBuf,
        pub cpus: usize,
        pub memory_mib: u64,
        pub cmdline: String,
        pub timeout: Duration,
    }

    pub fn dispatch(command: Command) -> Result<(), Error> {
        match command {
            Command::Info => info(),
            Command::BootLinux {
                kernel,
                initramfs,
                cpus,
                memory_mib,
                cmdline,
                timeout_secs,
            } => boot_linux(LinuxBoot {
                kernel,
                initramfs,
                cpus,
                memory_mib,
                cmdline,
                timeout: Duration::from_secs(timeout_secs),
            }),
        }
    }

    fn info() -> Result<(), Error> {
        let supported = unsafe { VZVirtualMachine::isSupported() };
        println!("virtualization_supported={supported}");
        if supported { Ok(()) } else { Err(Error::Unsupported) }
    }

    fn boot_linux(boot: LinuxBoot) -> Result<(), Error> {
        if !unsafe { VZVirtualMachine::isSupported() } {
            return Err(Error::Unsupported);
        }

        let timeout = boot.timeout;

        // Create and start the VM on the main queue, which is the queue VZ binds
        // the VM to by default. Building inside the block keeps every non-`Send`
        // objc2 object on that thread; the block captures only `boot`.
        DispatchQueue::main().exec_async(move || {
            let config = match build_linux_config(&boot) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("macos-vm: {error}");
                    std::process::exit(1);
                }
            };
            let vm = unsafe {
                VZVirtualMachine::initWithConfiguration(VZVirtualMachine::alloc(), &config)
            };
            let completion = RcBlock::new(|error: *mut NSError| {
                if error.is_null() {
                    eprintln!("macos-vm: guest started");
                } else {
                    // Safety: VZ hands us a valid retained NSError on failure.
                    let error = unsafe { &*error };
                    eprintln!("macos-vm: guest failed to start: {}", ns_error_message(error));
                    std::process::exit(1);
                }
            });
            unsafe { vm.startWithCompletionHandler(&completion) };
            // The VM must outlive this block: dropping the last `Retained` would
            // tear the running VM down. The process runs until the timeout, so
            // hand the VM to the process for its lifetime.
            std::mem::forget(vm);
        });

        // Hard stop so a background invocation never hangs: a separate thread
        // sleeps the timeout, then exits the process. The guest console has
        // streamed to stdout by then.
        std::thread::spawn(move || {
            std::thread::sleep(timeout);
            eprintln!("macos-vm: timeout reached, stopping");
            std::process::exit(0);
        });

        // Drains the main queue forever (`-> !`); the timeout thread ends the
        // process. Runs the boot block submitted above.
        dispatch_main();
    }

    fn build_linux_config(boot: &LinuxBoot) -> Result<Retained<VZVirtualMachineConfiguration>, Error> {
        let memory_bytes = boot
            .memory_mib
            .checked_mul(1024 * 1024)
            .ok_or(Error::MemoryTooLarge { mib: boot.memory_mib })?;

        let kernel_url = file_url(&boot.kernel);
        let initramfs_url = file_url(&boot.initramfs);

        let boot_loader =
            unsafe { VZLinuxBootLoader::initWithKernelURL(VZLinuxBootLoader::alloc(), &kernel_url) };
        unsafe {
            boot_loader.setInitialRamdiskURL(Some(&initramfs_url));
            boot_loader.setCommandLine(&NSString::from_str(&boot.cmdline));
        }

        // Guest serial console -> our stdout. VZ rejects a null read handle, so
        // give it the (unwritten) read end of a fresh pipe.
        let pipe = NSPipe::pipe();
        let read_handle = pipe.fileHandleForReading();
        let stdout_handle = NSFileHandle::fileHandleWithStandardOutput();
        let attachment = unsafe {
            VZFileHandleSerialPortAttachment::initWithFileHandleForReading_fileHandleForWriting(
                VZFileHandleSerialPortAttachment::alloc(),
                Some(&read_handle),
                Some(&stdout_handle),
            )
        };
        let serial = unsafe { VZVirtioConsoleDeviceSerialPortConfiguration::new() };
        let attachment_ref: &VZSerialPortAttachment = &attachment;
        unsafe { serial.setAttachment(Some(attachment_ref)) };

        let entropy = unsafe { VZVirtioEntropyDeviceConfiguration::new() };
        let balloon = unsafe { VZVirtioTraditionalMemoryBalloonDeviceConfiguration::new() };

        let config = unsafe { VZVirtualMachineConfiguration::new() };
        let boot_loader_ref: &VZBootLoader = &boot_loader;
        let serial_ref: &VZSerialPortConfiguration = &serial;
        let entropy_ref: &VZEntropyDeviceConfiguration = &entropy;
        let balloon_ref: &VZMemoryBalloonDeviceConfiguration = &balloon;
        unsafe {
            config.setBootLoader(Some(boot_loader_ref));
            config.setCPUCount(boot.cpus);
            config.setMemorySize(memory_bytes);
            config.setSerialPorts(&NSArray::from_slice(&[serial_ref]));
            config.setEntropyDevices(&NSArray::from_slice(&[entropy_ref]));
            config.setMemoryBalloonDevices(&NSArray::from_slice(&[balloon_ref]));
        }

        // Validation surfaces a missing entitlement as a clear error rather than
        // a later crash.
        if let Err(error) = unsafe { config.validateWithError() } {
            return Err(Error::InvalidConfiguration {
                message: ns_error_message(&error),
            });
        }

        Ok(config)
    }

    fn file_url(path: &std::path::Path) -> Retained<NSURL> {
        let s = NSString::from_str(&path.to_string_lossy());
        NSURL::fileURLWithPath(&s)
    }

    fn ns_error_message(error: &NSError) -> String {
        error.localizedDescription().to_string()
    }
}
