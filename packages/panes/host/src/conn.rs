//! Socket supervisor: connect with backoff, decode [`ToHost`] messages on a
//! reader thread and hand them to the main thread, drain outgoing [`ToGuest`]
//! messages on a writer thread. The `AppKit` main thread never touches the
//! socket, so a stalled guest can never hitch window presentation.

use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use crate::send_queue::{self, SendQueue, SendQueueReceiver};
use crate::transport::{Stream, Target, connect};
use dispatch2::DispatchQueue;
use panes_protocol::{
    Encoding, ToGuest, ToHost, VERSION_MAJOR, VERSION_MINOR, read_msg, write_msg,
};

/// What the supervisor tells the main thread. `Connected` carries the bounded
/// queue the main thread pushes outgoing messages into; replacing it on
/// `Disconnected` lets the writer side drain or exit.
pub enum Event {
    Connected(SendQueue),
    /// The guest's major-validated Hello. Its minor gates every 1.x message
    /// we emit (postcard has no unknown-variant tolerance, see the protocol
    /// crate), so the main thread must know it.
    Hello {
        minor: u16,
    },
    /// `recv` is the trace clock (`trace::now`) right after the wire decode
    /// on the reader thread, so `PANES_TRACE` frame lines can separate
    /// main-queue wait from ingest work; costs one timestamp per message.
    Msg {
        msg: ToHost,
        recv: f64,
    },
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

fn run_connection(stream: Stream, host: &HostInfo) {
    let Stream {
        read,
        write,
        shutdown,
    } = stream;
    // Outgoing traffic is bounded in a purpose-built queue: continuous pointer
    // and axis updates plus cumulative acks coalesce in place, while discrete
    // input keeps FIFO order or reports a broken connection instead of growing
    // without limit.
    let (tx, rx) = send_queue::channel_with_on_close(move || shutdown.shutdown());
    // The writer exits once the queue is closed by the main thread's
    // `Disconnected` replacement, by a full discrete FIFO, or by write
    // failure. No join needed; it owns nothing shared.
    std::thread::spawn(move || write_loop(write, rx));
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
    read_loop(read);
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
            Ok(msg) => post(Event::Msg {
                msg,
                recv: crate::trace::now(),
            }),
            Err(error) => {
                eprintln!("panes-host: connection lost: {error}");
                return;
            }
        }
    }
}

fn write_loop(write: Box<dyn Write + Send>, rx: SendQueueReceiver) {
    let mut writer = BufWriter::new(write);
    while let Some(msg) = rx.recv() {
        if write_msg(&mut writer, &msg).is_err() {
            return;
        }
        // Drain whatever queued while we were writing so one flush covers the
        // burst (a frame's worth of input events, acks, configures).
        while let Some(next) = rx.try_recv() {
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
