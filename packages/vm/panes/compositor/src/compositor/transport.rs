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

use std::io::{BufReader, BufWriter};
use std::io::Write as _;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context as _;
use panes_guest_transport::{Acceptor, Conn};
pub use panes_guest_transport::ListenSpec;
use panes_protocol::{ToGuest, ToHost, read_msg, write_msg};
use smithay::reexports::calloop::channel::Sender;
use tracing::{debug, info, warn};

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
    /// Host protocol minor from Hello: postcard has no unknown-variant
    /// tolerance, so 1.x messages are only emitted once this says the host
    /// decodes them.
    pub minor: u16,
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
        minor: 0,
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
