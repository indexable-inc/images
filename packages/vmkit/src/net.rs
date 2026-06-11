//! Userspace networking for the libkrun Linux guest.
//!
//! libkrun's default backend is TSI (transparent socket impersonation), which
//! needs a TSI-aware guest kernel: libkrun's bundled `libkrunfw` kernel (the
//! Linux-host path) has it, but a stock NixOS kernel booted from an EFI disk
//! (the macOS-host path) does not. So the two hosts get network two different
//! ways, the same split the rest of `linuxkrun` already has:
//!
//! - **Linux host** (classic libkrun + libkrunfw): TSI. Outbound works with no
//!   setup; inbound host->guest ports are exposed with `krun_set_port_map`
//!   (handled in `linuxkrun`).
//! - **macOS host** (libkrun-efi + stock guest kernel): a userspace proxy,
//!   `gvproxy` (gvisor-tap-vsock), the same one krunkit/podman-machine use. It
//!   gives the guest a virtio-net NIC over a unixgram socket
//!   (`krun_set_gvproxy_path`), NATs outbound traffic, and exposes guest ports
//!   back to the host through its HTTP forwarder control API. `krun_set_port_map`
//!   is TSI-only (returns `-ENOTSUP` under a proxy), so inbound forwarding is
//!   configured through gvproxy instead. [`Proxy::start`] spawns gvproxy and
//!   blocks until its sockets exist, since `krun_set_gvproxy_path` must run
//!   before `krun_start_enter` with the proxy already listening.

/// One host->guest TCP forward. The guest service is reached on the host at
/// `host` (gvproxy exposes the guest's `guest` port as the host's `host` port,
/// bound on all host interfaces so the VM is reachable like a normal server).
#[derive(Debug, Clone, Copy)]
pub struct Forward {
    pub host: u16,
    pub guest: u16,
}

/// Guest network configuration. A `BootLinux` with `net: None` keeps libkrun's
/// no-interface default; `Some` attaches a NIC and applies the forwards.
#[derive(Debug, Clone)]
pub struct Net {
    pub forwards: Vec<Forward>,
}

#[cfg(target_os = "macos")]
pub use macos::{Error, Proxy};

#[cfg(target_os = "macos")]
mod macos {
    use std::time::Duration;

    use snafu::{ResultExt, Snafu};

    use super::{Forward, Net};

    #[derive(Debug, Snafu)]
    pub enum Error {
        #[snafu(display("spawn gvproxy ({bin:?}): {source}"))]
        Spawn { bin: String, source: std::io::Error },
        #[snafu(display(
            "gvproxy did not create its sockets within {}s (it may have exited; check its stderr)",
            timeout.as_secs()
        ))]
        SocketTimeout { timeout: Duration },
        #[snafu(display("gvproxy exited before it was ready (status {status})"))]
        EarlyExit { status: String },
        #[snafu(display("create gvproxy socket dir: {source}"))]
        TempDir { source: std::io::Error },
        #[snafu(display("connect to gvproxy control socket {path:?}: {source}"))]
        ControlConnect {
            path: String,
            source: std::io::Error,
        },
        #[snafu(display("talk to gvproxy control socket: {source}"))]
        ControlIo { source: std::io::Error },
        #[snafu(display("gvproxy refused to expose {host}->{guest}: {response}"))]
        ExposeRejected {
            host: u16,
            guest: u16,
            response: String,
        },
    }

    /// gvproxy's fixed guest address on its default 192.168.127.0/24 network: the
    /// first DHCP lease. The guest NIC must DHCP (the nox-server guest image does)
    /// to take it.
    const GUEST_IP: &str = "192.168.127.2";

    /// The gvproxy binary. Resolved at build time to a Nix store path via
    /// `IX_VMKIT_GVPROXY`; falls back to `gvproxy` on `PATH` for a dev build.
    fn gvproxy_bin() -> String {
        std::env::var("IX_VMKIT_GVPROXY").unwrap_or_else(|_| "gvproxy".to_owned())
    }

    /// A running gvproxy and the unix-socket path libkrun must connect its NIC to.
    /// Dropping it (and the process-exit hook installed by [`Proxy::start`]) tears
    /// gvproxy down; `vmkit` outlives the VM, so the proxy lives exactly as long.
    pub struct Proxy {
        child: std::process::Child,
        vfkit_socket: std::path::PathBuf,
        _dir: tempfile::TempDir,
    }

    impl Proxy {
        /// Spawn gvproxy, wait until its vfkit + control sockets exist, and expose
        /// every forward. The returned [`Proxy`] must be kept alive until after
        /// `krun_start_enter`.
        pub fn start(net: &Net) -> Result<Self, Error> {
            let dir = tempfile::TempDir::with_prefix("ix-vmkit-net-").context(TempDirSnafu)?;
            let vfkit_socket = dir.path().join("vfkit.sock");
            let control_socket = dir.path().join("control.sock");

            let bin = gvproxy_bin();
            let child = std::process::Command::new(&bin)
                .arg("-listen")
                .arg(format!("unix://{}", control_socket.display()))
                .arg("-listen-vfkit")
                .arg(format!("unixgram://{}", vfkit_socket.display()))
                .stdin(std::process::Stdio::null())
                .spawn()
                .with_context(|_| SpawnSnafu { bin })?;

            let mut proxy = Self {
                child,
                vfkit_socket,
                _dir: dir,
            };
            proxy.wait_for_socket(&control_socket)?;
            // Kill gvproxy even though `krun_start_enter` `exit()`s without
            // unwinding (so Drop never runs on the serve path).
            register_exit_kill(proxy.child.id());
            for forward in &net.forwards {
                proxy.expose(&control_socket, *forward)?;
            }
            Ok(proxy)
        }

        /// The vfkit unixgram socket path to hand libkrun via `krun_set_gvproxy_path`.
        pub fn vfkit_socket(&self) -> &std::path::Path {
            &self.vfkit_socket
        }

        fn wait_for_socket(&mut self, control: &std::path::Path) -> Result<(), Error> {
            let timeout = Duration::from_secs(10);
            let start = std::time::Instant::now();
            while start.elapsed() < timeout {
                if control.exists() && self.vfkit_socket.exists() {
                    return Ok(());
                }
                if let Some(status) = self.child.try_wait().ok().flatten() {
                    return Err(Error::EarlyExit {
                        status: status.to_string(),
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(Error::SocketTimeout { timeout })
        }

        /// POST one forward to gvproxy's HTTP control API over its unix socket.
        /// gvproxy speaks plain HTTP/1.1 there, so a hand-written request avoids a
        /// hyper/unix-connector dependency for a single fixed call.
        fn expose(&self, control: &std::path::Path, forward: Forward) -> Result<(), Error> {
            use std::io::{Read, Write};

            let body = format!(
                r#"{{"local":":{host}","remote":"{GUEST_IP}:{guest}","protocol":"tcp"}}"#,
                host = forward.host,
                guest = forward.guest,
            );
            let request = format!(
                "POST /services/forwarder/expose HTTP/1.1\r\n\
                 Host: gvproxy\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {len}\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {body}",
                len = body.len(),
            );

            let mut stream =
                std::os::unix::net::UnixStream::connect(control).with_context(|_| {
                    ControlConnectSnafu {
                        path: control.display().to_string(),
                    }
                })?;
            stream
                .write_all(request.as_bytes())
                .context(ControlIoSnafu)?;
            let mut response = String::new();
            stream
                .read_to_string(&mut response)
                .context(ControlIoSnafu)?;

            let ok = response
                .lines()
                .next()
                .is_some_and(|status| status.contains(" 200") || status.contains(" 201"));
            if ok {
                Ok(())
            } else {
                Err(Error::ExposeRejected {
                    host: forward.host,
                    guest: forward.guest,
                    response: response
                        .lines()
                        .next()
                        .unwrap_or("<no response>")
                        .to_owned(),
                })
            }
        }
    }

    impl Drop for Proxy {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Kill gvproxy at process exit. `krun_start_enter` ends the process with a
    /// bare `exit()` (no unwinding), so [`Proxy`]'s `Drop` would never run on the
    /// serve path; an `atexit` hook covers it. The pid lives in an atomic the hook
    /// reads, since C `atexit` takes a bare `extern "C" fn`.
    fn register_exit_kill(pid: u32) {
        use std::sync::atomic::{AtomicI32, Ordering};
        static GVPROXY_PID: AtomicI32 = AtomicI32::new(0);
        static REGISTERED: std::sync::Once = std::sync::Once::new();

        let pid = i32::try_from(pid).unwrap_or(0);
        GVPROXY_PID.store(pid, Ordering::SeqCst);

        extern "C" fn kill_gvproxy() {
            let pid = GVPROXY_PID.load(Ordering::SeqCst);
            if pid > 0 {
                // SAFETY: SIGTERM to a pid we spawned; harmless if already reaped.
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        }
        REGISTERED.call_once(|| {
            // SAFETY: registering a plain extern "C" fn with no captured state; it
            // only reads an atomic and calls `kill`.
            unsafe {
                libc::atexit(kill_gvproxy);
            }
        });
    }
}
