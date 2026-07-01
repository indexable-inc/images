//! Host transport: one byte stream at a time, blocking-IO threads bridged
//! into calloop through a channel.
//!
//! Layout: an accept thread owns the listener; each accepted connection gets
//! a reader thread (decodes `ToGuest`, forwards into the event loop) and a
//! writer thread (drains an mpsc of `ToHost`). Blocking writer threads keep a
//! slow host from ever stalling Wayland dispatch, and `read_msg`/`write_msg`
//! are plain blocking calls so no nonblocking framing state machine is
//! needed. Events carry a connection generation so a stale thread's messages
//! are ignored after the main loop moved on.

use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context as _;
use panes_protocol::{ToGuest, ToHost, read_msg, write_msg};
use smithay::reexports::calloop::channel::Sender;
use tracing::{debug, info, warn};
use vsock::{VMADDR_CID_ANY, VsockListener, VsockStream};

/// Where to accept the (single) host connection.
pub enum ListenSpec {
    Vsock(u32),
    Unix(PathBuf),
    Tcp(String),
}

/// Transport-side events delivered into the compositor event loop.
pub enum HostEvent {
    Connected(HostLink),
    Message { generation: u64, msg: ToGuest },
    Disconnected { generation: u64 },
}

/// The compositor's handle on the live host connection. Protocol state
/// negotiated by `ToGuest::Hello` lives here so it drops with the connection.
pub struct HostLink {
    pub generation: u64,
    /// True once the host's Hello arrived; windows are announced only then.
    pub ready: bool,
    /// Host advertised `Encoding::Lz4` in Hello (Raw is always legal).
    pub lz4: bool,
    /// Host backingScaleFactor from Hello.
    pub scale: u32,
    tx: mpsc::Sender<ToHost>,
    /// A clone of the socket kept purely to force-shutdown a connection we
    /// refuse (second host) or that failed the version handshake; dropping
    /// `tx` alone would leave the reader thread parked in `read_msg`.
    conn: Box<dyn Conn>,
}

impl HostLink {
    /// Queue a message for the writer thread. A send after the writer died is
    /// dropped silently: the Disconnected event is already on its way and the
    /// compositor will re-announce everything on the next connection.
    pub fn send(&self, msg: ToHost) {
        if self.tx.send(msg).is_err() {
            debug!("host writer gone; message dropped");
        }
    }

    pub fn close(&self) {
        self.conn.shutdown_conn();
    }
}

/// Object-safe stream: vsock, unix, and TCP only share Read/Write, but the
/// per-connection threads each need their own handle plus a shutdown lever.
trait Conn: Read + Write + Send {
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>>;
    fn shutdown_conn(&self);
}

impl Conn for TcpStream {
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn shutdown_conn(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

impl Conn for UnixStream {
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn shutdown_conn(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

impl Conn for VsockStream {
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn shutdown_conn(&self) {
        let _ = self.shutdown(Shutdown::Both);
    }
}

enum Acceptor {
    Vsock(VsockListener),
    Unix(UnixListener),
    Tcp(TcpListener),
}

impl Acceptor {
    fn bind(spec: &ListenSpec) -> anyhow::Result<Self> {
        match spec {
            ListenSpec::Vsock(port) => {
                let listener = VsockListener::bind_with_cid_port(VMADDR_CID_ANY, *port)
                    .with_context(|| format!("bind vsock port {port}"))?;
                info!(port, "listening on vsock");
                Ok(Self::Vsock(listener))
            }
            ListenSpec::Unix(path) => {
                // A previous run's socket file would fail the bind with
                // EADDRINUSE even though nothing is listening.
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(err) if err.kind() == ErrorKind::NotFound => {}
                    Err(err) => {
                        return Err(err)
                            .with_context(|| format!("remove stale {}", path.display()));
                    }
                }
                let listener = UnixListener::bind(path)
                    .with_context(|| format!("bind unix socket {}", path.display()))?;
                info!(path = %path.display(), "listening on unix socket");
                Ok(Self::Unix(listener))
            }
            ListenSpec::Tcp(addr) => {
                let listener =
                    TcpListener::bind(addr).with_context(|| format!("bind tcp {addr}"))?;
                info!(addr, "listening on tcp");
                Ok(Self::Tcp(listener))
            }
        }
    }

    fn accept(&self) -> std::io::Result<Box<dyn Conn>> {
        match self {
            Self::Vsock(listener) => {
                let (stream, _addr) = listener.accept()?;
                Ok(Box::new(stream))
            }
            Self::Unix(listener) => {
                let (stream, _addr) = listener.accept()?;
                Ok(Box::new(stream))
            }
            Self::Tcp(listener) => {
                let (stream, _addr) = listener.accept()?;
                Ok(Box::new(stream))
            }
        }
    }
}

/// Bind (fatal at startup so a misconfigured transport fails loudly) and hand
/// the listener to a detached accept thread feeding `events`.
pub fn spawn(spec: &ListenSpec, events: Sender<HostEvent>) -> anyhow::Result<()> {
    let acceptor = Acceptor::bind(spec)?;
    std::thread::Builder::new()
        .name("panes-accept".into())
        .spawn(move || accept_loop(&acceptor, &events))
        .context("spawn accept thread")?;
    Ok(())
}

fn accept_loop(acceptor: &Acceptor, events: &Sender<HostEvent>) {
    let mut generation = 0_u64;
    loop {
        let conn = match acceptor.accept() {
            Ok(conn) => conn,
            Err(err) => {
                // Transient accept errors (EMFILE, aborted handshakes) must
                // not kill the only accept loop; back off and retry.
                warn!(%err, "accept failed; retrying");
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        };
        generation += 1;
        let link = match wire_up(conn, generation, events) {
            Ok(link) => link,
            Err(err) => {
                warn!(%err, "failed to set up host connection; dropping it");
                continue;
            }
        };
        info!(generation, "host connected");
        if events.send(HostEvent::Connected(link)).is_err() {
            // The event loop is gone; nothing left to accept for.
            return;
        }
    }
}

fn wire_up(
    conn: Box<dyn Conn>,
    generation: u64,
    events: &Sender<HostEvent>,
) -> anyhow::Result<HostLink> {
    let (tx, rx) = mpsc::channel::<ToHost>();
    let writer = conn.try_clone_conn().context("clone for writer")?;
    let writer_events = events.clone();
    std::thread::Builder::new()
        .name(format!("panes-write-{generation}"))
        .spawn(move || writer_loop(writer, &rx, &writer_events, generation))
        .context("spawn writer thread")?;
    let reader = conn.try_clone_conn().context("clone for reader")?;
    let reader_events = events.clone();
    std::thread::Builder::new()
        .name(format!("panes-read-{generation}"))
        .spawn(move || reader_loop(reader, &reader_events, generation))
        .context("spawn reader thread")?;
    Ok(HostLink {
        generation,
        ready: false,
        lz4: false,
        scale: 1,
        tx,
        conn,
    })
}

fn reader_loop(conn: Box<dyn Conn>, events: &Sender<HostEvent>, generation: u64) {
    let mut reader = BufReader::new(conn);
    loop {
        match read_msg::<ToGuest>(&mut reader) {
            Ok(msg) => {
                if events.send(HostEvent::Message { generation, msg }).is_err() {
                    return;
                }
            }
            Err(err) => {
                debug!(generation, %err, "host read ended");
                let _ = events.send(HostEvent::Disconnected { generation });
                return;
            }
        }
    }
}

fn writer_loop(
    conn: Box<dyn Conn>,
    rx: &mpsc::Receiver<ToHost>,
    events: &Sender<HostEvent>,
    generation: u64,
) {
    let mut writer = BufWriter::new(conn);
    // Ends when the HostLink (and its mpsc Sender) is dropped or the socket
    // errors. Flush per message: frames are the latency-critical payload and
    // BufWriter only exists to coalesce the length prefix with the body.
    while let Ok(msg) = rx.recv() {
        let sent = write_msg(&mut writer, &msg).is_ok() && writer.flush().is_ok();
        if !sent {
            debug!(generation, "host write failed");
            let _ = events.send(HostEvent::Disconnected { generation });
            return;
        }
    }
}
