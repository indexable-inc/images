//! Socket supervisor: connect with backoff, decode [`ToHost`] messages on a
//! reader thread and hand them to the main thread, drain outgoing [`ToGuest`]
//! messages on a writer thread. The `AppKit` main thread never touches the
//! socket, so a stalled guest can never hitch window presentation.

use std::io::{BufReader, BufWriter, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
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
    Msg(ToHost),
    Disconnected,
}

/// Host facts advertised in [`ToGuest::Hello`], captured from `NSScreen` on
/// the main thread before the supervisor starts.
pub struct HostInfo {
    pub refresh_mhz: u32,
    pub scale: u32,
}

const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

pub fn spawn(target: Target, host: HostInfo) {
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
        refresh_mhz: host.refresh_mhz,
        scale: host.scale,
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
                } else {
                    eprintln!(
                        "panes-host: guest protocol major {major} != {VERSION_MAJOR}, hanging up"
                    );
                    return;
                }
            }
            Ok(msg) => post(Event::Msg(msg)),
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
