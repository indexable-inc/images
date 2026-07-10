//! Shared blocking listener transport for panes guest daemons.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

use anyhow::Context as _;
use tracing::info;
#[cfg(target_os = "linux")]
use vsock::{VMADDR_CID_ANY, VsockListener, VsockStream};

/// Where a guest daemon accepts its single host connection.
pub enum ListenSpec {
    /// AF_VSOCK port in production. Binding it outside Linux returns an error.
    Vsock(u32),
    /// Unix socket path for local development.
    Unix(PathBuf),
    /// TCP address for portable development and tests.
    Tcp(String),
}

/// Object-safe stream with cloning and whole-socket shutdown.
pub trait Conn: Read + Write + Send {
    /// Clone a handle for an independent blocking reader or writer.
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>>;
    /// Unblock every cloned handle and close both directions.
    fn shutdown_conn(&self);
}

macro_rules! impl_conn {
    ($stream:ty) => {
        impl Conn for $stream {
            fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>> {
                Ok(Box::new(self.try_clone()?))
            }

            fn shutdown_conn(&self) {
                let _ = self.shutdown(Shutdown::Both);
            }
        }
    };
}

impl_conn!(TcpStream);
impl_conn!(UnixStream);
#[cfg(target_os = "linux")]
impl_conn!(VsockStream);

/// Bound listener for any supported guest transport.
pub enum Acceptor {
    #[cfg(target_os = "linux")]
    Vsock(VsockListener),
    Unix(UnixListener),
    Tcp(TcpListener),
}

impl Acceptor {
    /// Bind a listener, removing a stale Unix socket file first.
    ///
    /// # Errors
    /// Returns the contextual bind or stale-file removal error.
    pub fn bind(spec: &ListenSpec) -> anyhow::Result<Self> {
        match spec {
            ListenSpec::Vsock(port) => bind_vsock(*port),
            ListenSpec::Unix(path) => {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("remove stale {}", path.display()));
                    }
                }
                let listener = UnixListener::bind(path)
                    .with_context(|| format!("bind unix socket {}", path.display()))?;
                info!(path = %path.display(), "listening on unix socket");
                Ok(Self::Unix(listener))
            }
            ListenSpec::Tcp(address) => {
                let listener = TcpListener::bind(address)
                    .with_context(|| format!("bind tcp {address}"))?;
                info!(address, "listening on tcp");
                Ok(Self::Tcp(listener))
            }
        }
    }

    /// Accept one host connection.
    ///
    /// # Errors
    /// Returns the listener's transient accept error.
    pub fn accept(&self) -> std::io::Result<Box<dyn Conn>> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Vsock(listener) => listener.accept().map(|(stream, _)| Box::new(stream) as _),
            Self::Unix(listener) => listener.accept().map(|(stream, _)| Box::new(stream) as _),
            Self::Tcp(listener) => listener.accept().map(|(stream, _)| Box::new(stream) as _),
        }
    }
}

#[cfg(target_os = "linux")]
fn bind_vsock(port: u32) -> anyhow::Result<Acceptor> {
    let listener = VsockListener::bind_with_cid_port(VMADDR_CID_ANY, port)
        .with_context(|| format!("bind vsock port {port}"))?;
    info!(port, "listening on vsock");
    Ok(Acceptor::Vsock(listener))
}

#[cfg(not(target_os = "linux"))]
fn bind_vsock(_port: u32) -> anyhow::Result<Acceptor> {
    anyhow::bail!("AF_VSOCK is Linux-only; use a Unix or TCP development listener")
}
