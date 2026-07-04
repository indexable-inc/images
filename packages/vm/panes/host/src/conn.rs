//! Socket supervisor: connect with backoff, decode [`ToHost`] messages on a
//! reader thread and hand them to the main thread, drain outgoing [`ToGuest`]
//! messages on a writer thread. The `AppKit` main thread never touches the
//! socket, so a stalled guest can never hitch window presentation.

use std::io::{BufReader, BufWriter, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use dispatch2::DispatchQueue;
use panes_protocol::{Encoding, ToGuest, ToHost, VERSION_MAJOR, VERSION_MINOR, read_msg, write_msg};

pub enum Target {
    Unix(PathBuf),
    Tcp(String),
}

/// What the supervisor tells the main thread. `Connected` carries the sender
/// the main thread queues outgoing messages on; dropping it (on `Disconnected`)
/// is what lets the writer thread exit.
pub enum Event {
    Connected(mpsc::Sender<ToGuest>),
    /// The guest's major-validated Hello. Its minor gates every 1.x message
    /// we emit (postcard has no unknown-variant tolerance, see the protocol
    /// crate), so the main thread must know it.
    Hello { minor: u16 },
    /// `recv` is the trace clock (`trace::now`) right after the wire decode
    /// on the reader thread, so `PANES_TRACE` frame lines can separate
    /// main-queue wait from ingest work; costs one timestamp per message.
    Msg { msg: ToHost, recv: f64 },
    Disconnected,
}

/// Host facts advertised in [`ToGuest::Hello`]. Read from `NSScreen` on the
/// main thread, and re-written there on every screen-parameters change
/// (displays attach/detach/change mode mid-session); the supervisor loads
/// the current values at each (re)connect, so a Hello sent after a display
/// change advertises the topology that exists, not the one from launch.
pub struct HostInfo {
    /// Main-screen refresh in mHz (e.g. 120000 for `ProMotion`).
    pub refresh_mhz: AtomicU32,
    /// Highest `backingScaleFactor` of any attached display.
    pub scale: AtomicU32,
}

const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

pub fn spawn(target: Target, host: Arc<HostInfo>) {
    std::thread::spawn(move || supervise(&target, &host));
}

fn supervise(target: &Target, host: &HostInfo) -> ! {
    let mut backoff = BACKOFF_START;
    loop {
        match connect(target) {
            Ok(stream) => {
                backoff = BACKOFF_START;
                run_connection(stream, host);
                post(Event::Disconnected);
            }
            Err(error) => eprintln!("panes-host: connect failed: {error}"),
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

struct Stream {
    read: Box<dyn Read + Send>,
    write: Box<dyn Write + Send>,
}

fn connect(target: &Target) -> std::io::Result<Stream> {
    match target {
        Target::Unix(path) => {
            let stream = UnixStream::connect(path)?;
            let read = stream.try_clone()?;
            Ok(Stream { read: Box::new(read), write: Box::new(stream) })
        }
        Target::Tcp(addr) => {
            let stream = TcpStream::connect(addr.as_str())?;
            // Acks pace the guest's next frame; Nagle batching them would cap
            // the loop well under the display rate.
            stream.set_nodelay(true)?;
            let read = stream.try_clone()?;
            Ok(Stream { read: Box::new(read), write: Box::new(stream) })
        }
    }
}

fn run_connection(stream: Stream, host: &HostInfo) {
    // TODO(review P2): this outbound queue is unbounded, so a connected but
    // stalled peer accumulates messages (pointer motion dominates) until it
    // drains. Bounding it needs drop-oldest semantics for coalescable
    // traffic (motion/axis/ack) while never dropping CloseRequest/Configure/
    // Key, so it wants a small purpose-built queue rather than mpsc;
    // deferred until the real compositor exists to test against.
    let (tx, rx) = mpsc::channel::<ToGuest>();
    // The writer exits once every sender is gone: ours right below, the main
    // thread's on `Disconnected`. No join needed; it owns nothing shared.
    std::thread::spawn(move || write_loop(stream.write, &rx));
    // Hello goes out before the main thread learns of the connection so the
    // encoding advertisement precedes anything else on the wire.
    let hello = ToGuest::Hello {
        major: VERSION_MAJOR,
        minor: VERSION_MINOR,
        // Relaxed: single u32 facts, no ordering relationship between them
        // worth paying for (each is independently valid slightly stale).
        refresh_mhz: host.refresh_mhz.load(Ordering::Relaxed),
        scale: host.scale.load(Ordering::Relaxed),
        encodings: vec![Encoding::Raw, Encoding::Lz4],
    };
    if tx.send(hello).is_err() {
        return;
    }
    post(Event::Connected(tx));
    read_loop(stream.read);
}

/// Read until EOF/error or a version-mismatched Hello (protocol says: refuse
/// and hang up; dropping both stream halves is the hangup).
fn read_loop(read: Box<dyn Read + Send>) {
    let mut reader = BufReader::new(read);
    loop {
        match read_msg::<ToHost>(&mut reader) {
            Ok(ToHost::Hello { major, minor }) => {
                if major == VERSION_MAJOR {
                    eprintln!("panes-host: guest speaks protocol {major}.{minor}");
                    post(Event::Hello { minor });
                } else {
                    eprintln!(
                        "panes-host: guest protocol major {major} != {VERSION_MAJOR}, hanging up"
                    );
                    return;
                }
            }
            Ok(msg) => post(Event::Msg { msg, recv: crate::trace::now() }),
            Err(error) => {
                eprintln!("panes-host: connection lost: {error}");
                return;
            }
        }
    }
}

fn write_loop(write: Box<dyn Write + Send>, rx: &mpsc::Receiver<ToGuest>) {
    let mut writer = BufWriter::new(write);
    while let Ok(msg) = rx.recv() {
        if write_msg(&mut writer, &msg).is_err() {
            return;
        }
        // Drain whatever queued while we were writing so one flush covers the
        // burst (a frame's worth of input events, acks, configures).
        while let Ok(next) = rx.try_recv() {
            if write_msg(&mut writer, &next).is_err() {
                return;
            }
        }
        if writer.flush().is_err() {
            return;
        }
    }
}

fn post(event: Event) {
    // All window state lives in a main-thread thread_local; the main dispatch
    // queue is the one serialization point (mirrors vmkit's discipline).
    DispatchQueue::main().exec_async(move || crate::app::on_event(event));
}
