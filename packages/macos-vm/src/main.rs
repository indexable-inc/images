//! `macos-vm`: drive Apple's Virtualization.framework from Rust.
//!
//! This binary owns a VM's lifecycle so that callers that cannot hold the
//! `com.apple.security.virtualization` entitlement themselves (notably the
//! ix-mcp Python interpreter, an unsigned immutable Nix store binary) can spawn
//! it and control a VM over IPC. The entitlement lives on *this* signed
//! process, never on the interpreter.
//!
//! v1 surface:
//!   * `macos-vm info`        — report whether virtualization is available.
//!   * `macos-vm boot-linux`  — boot a Linux guest from a raw kernel `Image`
//!                              and initramfs, streaming the guest serial
//!                              console to stdout. This is the end-to-end smoke
//!                              path: a real guest reaching userspace proves the
//!                              binding, the entitlement, and the boot work.
//!
//! The graphics/screenshot, vsock IPC, OCI-disk, and macOS-guest paths build on
//! the same `VZVirtualMachineConfiguration` and are tracked in the README.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "macos-vm",
    about = "Drive Apple's Virtualization.framework from Rust"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("macos-vm: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "macos")]
fn run(command: Command) -> Result<(), imp::Error> {
    match command {
        Command::Info => imp::info(),
        Command::BootLinux {
            kernel,
            initramfs,
            cpus,
            memory_mib,
            cmdline,
            timeout_secs,
        } => imp::boot_linux(imp::LinuxBoot {
            kernel,
            initramfs,
            cpus,
            memory_bytes: memory_mib * 1024 * 1024,
            cmdline,
            timeout: std::time::Duration::from_secs(timeout_secs),
        }),
    }
}

/// On non-macOS targets the binary still builds (so the Linux CI workspace graph
/// stays green) but every command is a typed refusal rather than a silent
/// fallback.
#[cfg(not(target_os = "macos"))]
fn run(_command: Command) -> Result<(), NotMacOs> {
    Err(NotMacOs)
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, snafu::Snafu)]
#[snafu(display("macos-vm requires macOS and Apple's Virtualization.framework"))]
struct NotMacOs;

#[cfg(target_os = "macos")]
mod imp {
    //! The Virtualization.framework glue. Everything here runs on the process
    //! main thread, which is the queue VZ binds the VM to by default; the guest
    //! vCPUs run on framework-owned threads, and `dispatch_main` drains the main
    //! queue so VZ's completion handlers fire (mirroring Apple's sample app,
    //! which wraps the same calls in `dispatchMain()`).

    use std::path::PathBuf;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSArray, NSError, NSFileHandle, NSPipe, NSString, NSURL};
    use objc2_virtualization::{
        VZBootLoader, VZEntropyDeviceConfiguration, VZFileHandleSerialPortAttachment,
        VZLinuxBootLoader, VZMemoryBalloonDeviceConfiguration, VZSerialPortAttachment,
        VZSerialPortConfiguration, VZVirtioConsoleDeviceSerialPortConfiguration,
        VZVirtioEntropyDeviceConfiguration, VZVirtioTraditionalMemoryBalloonDeviceConfiguration,
        VZVirtualMachine, VZVirtualMachineConfiguration,
    };
    use snafu::Snafu;

    #[derive(Debug, Snafu)]
    pub enum Error {
        #[snafu(display("virtualization is not available on this host"))]
        Unsupported,
        #[snafu(display(
            "virtual machine configuration is invalid: {message} \
             (an unsigned binary, or one missing com.apple.security.virtualization, \
             fails configuration validation)"
        ))]
        InvalidConfiguration { message: String },
        #[snafu(display("failed to start the virtual machine: {message}"))]
        StartFailed { message: String },
    }

    /// Parameters for a Linux guest boot. A named struct rather than a wide
    /// tuple so callers (and the future IPC layer) name each field.
    pub struct LinuxBoot {
        pub kernel: PathBuf,
        pub initramfs: PathBuf,
        pub cpus: usize,
        pub memory_bytes: u64,
        pub cmdline: String,
        pub timeout: Duration,
    }

    pub fn info() -> Result<(), Error> {
        let supported = unsafe { VZVirtualMachine::isSupported() };
        println!("virtualization_supported={supported}");
        if supported {
            Ok(())
        } else {
            Err(Error::Unsupported)
        }
    }

    pub fn boot_linux(boot: LinuxBoot) -> Result<(), Error> {
        if !unsafe { VZVirtualMachine::isSupported() } {
            return Err(Error::Unsupported);
        }

        let config = build_linux_config(&boot)?;

        // Validate before constructing the VM: this is where a missing
        // entitlement surfaces as a clear error instead of a later crash.
        if let Err(error) = unsafe { config.validateWithError() } {
            return Err(Error::InvalidConfiguration {
                message: ns_error_message(&error),
            });
        }

        let vm = unsafe { VZVirtualMachine::initWithConfiguration(VZVirtualMachine::alloc(), &config) };

        // Completion handler runs on the main queue once `dispatch_main` drains
        // it. The error pointer is null on success.
        let completion = RcBlock::new(|error: *mut NSError| {
            if error.is_null() {
                eprintln!("macos-vm: guest started");
            } else {
                // Safety: VZ hands us a valid, retained NSError on failure.
                let error = unsafe { &*error };
                eprintln!("macos-vm: guest failed to start: {}", ns_error_message(error));
                std::process::exit(1);
            }
        });
        unsafe { vm.startWithCompletionHandler(&completion) };

        // Hard stop so a background invocation never hangs: a separate thread
        // sleeps the timeout, then exits the process. The guest console has
        // already streamed to stdout by then.
        let timeout = boot.timeout;
        std::thread::spawn(move || {
            std::thread::sleep(timeout);
            eprintln!("macos-vm: timeout reached, stopping");
            std::process::exit(0);
        });

        // Drains the main queue forever; the timeout thread ends the process.
        dispatch2::dispatch_main();
    }

    fn build_linux_config(boot: &LinuxBoot) -> Result<objc2::rc::Retained<VZVirtualMachineConfiguration>, Error> {
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
            config.setMemorySize(boot.memory_bytes);
            config.setSerialPorts(&NSArray::from_slice(&[serial_ref]));
            config.setEntropyDevices(&NSArray::from_slice(&[entropy_ref]));
            config.setMemoryBalloonDevices(&NSArray::from_slice(&[balloon_ref]));
        }
        Ok(config)
    }

    fn file_url(path: &std::path::Path) -> objc2::rc::Retained<NSURL> {
        let s = NSString::from_str(&path.to_string_lossy());
        NSURL::fileURLWithPath(&s)
    }

    fn ns_error_message(error: &NSError) -> String {
        error.localizedDescription().to_string()
    }
}
