//! Build + e2e driver for the four metal-httpd backends.
//!
//! `cargo xtask build [backend...]` produces each backend's bootable
//! artifact; `cargo xtask e2e [backend...]` additionally launches it (host
//! process for linux/eyra, QEMU for hermit/bare), sends a real HTTP request
//! from the host, and asserts the response names the backend that served it.

use std::env;
use std::fmt;
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use bootloader::DiskImageBuilder;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Backend {
    /// std on glibc — the baseline everything else is compared against.
    Linux,
    /// std with libc replaced by the pure-Rust Eyra implementation.
    Eyra,
    /// std on the Hermit unikernel; kernel and app are one QEMU image.
    Hermit,
    /// Freestanding kernel with its own virtio-net driver + smoltcp.
    Bare,
}

const ALL: [Backend; 4] = [Backend::Linux, Backend::Eyra, Backend::Hermit, Backend::Bare];

/// How long each backend gets from launch to a correct HTTP response.
/// QEMU-under-TCG boots (especially hermit's DHCP) need the long tail.
fn deadline(backend: Backend) -> Duration {
    match backend {
        Backend::Linux | Backend::Eyra => Duration::from_secs(10),
        Backend::Hermit | Backend::Bare => Duration::from_secs(90),
    }
}

impl Backend {
    fn name(self) -> &'static str {
        match self {
            Backend::Linux => "linux",
            Backend::Eyra => "eyra",
            Backend::Hermit => "hermit",
            Backend::Bare => "bare",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        ALL.into_iter().find(|b| b.name() == s)
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let (command, rest) = match args.split_first() {
        Some((cmd, rest)) => (cmd.as_str(), rest),
        None => usage(),
    };
    let backends: Vec<Backend> = if rest.is_empty() {
        ALL.to_vec()
    } else {
        rest.iter()
            .map(|s| Backend::parse(s).unwrap_or_else(|| usage()))
            .collect()
    };

    match command {
        "build" => {
            for backend in backends {
                let artifact = build(backend);
                println!("[{backend}] artifact: {}", artifact.display());
            }
        }
        "e2e" => {
            let mut failures = Vec::new();
            for backend in backends {
                println!("=== e2e: {backend} ===");
                match e2e(backend) {
                    Ok(body) => println!("[{backend}] OK\n{body}"),
                    Err(err) => {
                        eprintln!("[{backend}] FAILED: {err}");
                        failures.push(backend);
                    }
                }
            }
            if !failures.is_empty() {
                let names: Vec<_> = failures.iter().map(|b| b.name()).collect();
                eprintln!("e2e failures: {}", names.join(", "));
                std::process::exit(1);
            }
            println!("e2e: all backends passed");
        }
        _ => usage(),
    }
}

fn usage() -> ! {
    eprintln!("usage: cargo xtask <build|e2e> [linux|eyra|hermit|bare ...]");
    std::process::exit(2);
}

/// The metal-httpd workspace root (parent of xtask/).
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Per-backend output directory for artifacts and serial logs.
fn e2e_dir(backend: Backend) -> PathBuf {
    let dir = workspace().join("target/e2e").join(backend.name());
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn cargo() -> Command {
    let mut cmd = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    cmd.current_dir(workspace());
    cmd
}

fn run(cmd: &mut Command, what: &str) {
    let status = cmd.status().unwrap_or_else(|e| panic!("failed to spawn {what}: {e}"));
    assert!(status.success(), "{what} failed with {status}");
}

/// Builds one backend and returns the launchable artifact: the server binary
/// for linux/eyra/hermit, the bootable disk image for bare.
fn build(backend: Backend) -> PathBuf {
    let ws = workspace();
    // linux and eyra are feature flavors of the same crate and would
    // overwrite each other in target/release, so each backend's artifact is
    // copied to its own directory.
    let artifact = e2e_dir(backend).join(if backend == Backend::Bare {
        "disk.img"
    } else {
        "httpd"
    });
    match backend {
        Backend::Linux => {
            run(
                cargo().args(["build", "-p", "httpd", "--release"]),
                "cargo build (linux)",
            );
            std::fs::copy(ws.join("target/release/httpd"), &artifact).unwrap();
        }
        Backend::Eyra => {
            run(
                cargo().args(["build", "-p", "httpd", "--release", "--features", "eyra"]),
                "cargo build (eyra)",
            );
            std::fs::copy(ws.join("target/release/httpd"), &artifact).unwrap();
        }
        Backend::Hermit => {
            run(
                cargo().args([
                    "build",
                    "-p",
                    "httpd",
                    "--release",
                    "--target",
                    "x86_64-unknown-hermit",
                    "-Zbuild-std=std,panic_abort",
                ]),
                "cargo build (hermit)",
            );
            std::fs::copy(
                ws.join("target/x86_64-unknown-hermit/release/httpd"),
                &artifact,
            )
            .unwrap();
        }
        Backend::Bare => {
            run(
                cargo().args([
                    "build",
                    "-p",
                    "kernel",
                    "--release",
                    "--target",
                    "x86_64-unknown-none",
                ]),
                "cargo build (kernel)",
            );
            DiskImageBuilder::new(ws.join("target/x86_64-unknown-none/release/kernel"))
                .create_bios_image(&artifact)
                .expect("creating BIOS disk image failed");
        }
    }
    artifact
}

/// Hermit is a library OS: the app image still needs a bootloader. Uses
/// `HERMIT_LOADER` if set; otherwise clones and builds the hermit-loader
/// release matching hermit kernel 0.12 (cached under target/).
fn hermit_loader() -> PathBuf {
    if let Some(path) = env::var_os("HERMIT_LOADER") {
        return path.into();
    }
    let checkout = workspace().join("target/hermit-loader");
    let loader = checkout.join("target/release/hermit-loader-x86_64");
    if loader.exists() {
        return loader;
    }
    if !checkout.join("Cargo.toml").exists() {
        run(
            Command::new("git").args([
                "clone",
                "--depth",
                "1",
                "--branch",
                "v0.5.6",
                "https://github.com/hermit-os/loader",
                checkout.to_str().unwrap(),
            ]),
            "git clone hermit-loader",
        );
    }
    run(
        cargo()
            .current_dir(&checkout)
            .args(["run", "--package=xtask", "--", "build", "--target", "x86_64", "--release"]),
        "hermit-loader build",
    );
    assert!(loader.exists(), "loader missing after build: {}", loader.display());
    loader
}

/// A child process that dies with the harness (also on panic/ctrl-c).
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn e2e(backend: Backend) -> Result<String, String> {
    let artifact = build(backend);
    let port = free_port();
    let serial_log = e2e_dir(backend).join("serial.log");

    let child = match backend {
        Backend::Linux | Backend::Eyra => Command::new(&artifact)
            .env("PORT", port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", artifact.display()))?,
        Backend::Hermit => {
            let loader = hermit_loader();
            let mut cmd = qemu(port, &serial_log);
            // Hermit's scheduler requires fsgsbase+xsave, which QEMU's
            // default CPU model lacks.
            cmd.args(["-cpu", "qemu64,fsgsbase,rdtscp,xsave,xsaveopt"])
                .arg("-kernel")
                .arg(&loader)
                .arg("-initrd")
                .arg(&artifact);
            cmd.spawn().map_err(|e| format!("spawn qemu: {e}"))?
        }
        Backend::Bare => {
            let mut cmd = qemu(port, &serial_log);
            cmd.arg("-drive")
                .arg(format!("format=raw,file={}", artifact.display()))
                // Lets the kernel's panic handler terminate QEMU instead of
                // hanging the harness until the deadline.
                .args(["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"]);
            cmd.spawn().map_err(|e| format!("spawn qemu: {e}"))?
        }
    };
    let mut guard = Guard(child);

    let expect = format!("backend: {}", backend.name());
    let deadline = Instant::now() + deadline(backend);
    let mut last_err = String::from("no attempt made");
    while Instant::now() < deadline {
        if let Some(status) = guard.0.try_wait().ok().flatten() {
            return Err(format!(
                "server exited early with {status}{}",
                serial_tail(&serial_log)
            ));
        }
        match http_get(port) {
            Ok(response) if response.contains("HTTP/1.1 200") && response.contains(&expect) => {
                return Ok(response
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body.trim().to_string())
                    .unwrap_or(response));
            }
            Ok(response) => {
                return Err(format!("unexpected response:\n{response}"));
            }
            Err(e) => last_err = e,
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(format!("timed out; last error: {last_err}{}", serial_tail(&serial_log)))
}

/// Common QEMU setup: headless, serial to a log file, user-mode networking
/// with the guest's port 8080 forwarded to `port` on the host loopback, and
/// a modern-only virtio-net NIC.
fn qemu(port: u16, serial_log: &Path) -> Command {
    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-smp", "1", "-m", "512M", "-display", "none"])
        .arg("-serial")
        .arg(format!("file:{}", serial_log.display()))
        .args([
            "-netdev",
            &format!("user,id=u1,hostfwd=tcp:127.0.0.1:{port}-:8080"),
            "-device",
            "virtio-net-pci,netdev=u1,disable-legacy=on,disable-modern=off",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd
}

fn serial_tail(serial_log: &Path) -> String {
    match std::fs::read_to_string(serial_log) {
        Ok(log) if !log.trim().is_empty() => {
            let tail: Vec<&str> = log.lines().rev().take(15).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            format!("\n--- serial log tail ---\n{}", tail.join("\n"))
        }
        _ => String::new(),
    }
}

/// Asks the kernel for a free loopback port. The listener is dropped before
/// the server launches; the tiny reuse race doesn't matter for a test tool.
fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn http_get(port: u16) -> Result<String, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| format!("connect: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(b"GET / HTTP/1.1\r\nhost: e2e\r\nconnection: close\r\n\r\n")
        .map_err(|e| format!("write: {e}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("read: {e}"))?;
    if response.is_empty() {
        return Err("empty response (connection accepted but nothing served)".into());
    }
    Ok(response)
}
