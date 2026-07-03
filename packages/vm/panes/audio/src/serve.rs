//! Accept loop and per-connection pump.
//!
//! Layout: the accept loop serves ONE host connection at a time (panes has
//! exactly one host agent; a second connect waits in the OS backlog until the
//! current connection dies). Per connection: Hellos are exchanged first (both
//! sides send immediately on connect, majors validated before any PCM), then
//! a watchdog thread parks in `read_msg` to notice the host hanging up, while
//! this thread pumps the `PipeWire` tap into framed [`ToHost::Pcm`] messages.
//! The watchdog exists because the pump would otherwise only notice a dead
//! host on its next write -- and while `PipeWire` is down the pump writes
//! nothing, which would wedge the accept loop behind a corpse.

use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context as _;
use panes_protocol::audio::{self, SampleFormat, ToGuest, ToHost};
use panes_protocol::{WireError, read_msg_bounded, write_msg};
use tracing::{debug, info, warn};
#[cfg(target_os = "linux")]
use vsock::{VMADDR_CID_ANY, VsockListener, VsockStream};

/// Where to accept the (single) host connection.
pub enum ListenSpec {
    /// `AF_VSOCK` port, the production transport. The variant exists on
    /// every platform (so the CLI needs no cfg), but binding it outside
    /// Linux fails with a legible error: the daemon has no host-side role
    /// there, only the unix/TCP dev listeners.
    Vsock(u32),
    Unix(PathBuf),
    Tcp(String),
}

/// The fixed stream format advertised in the audio Hello. The sample format
/// is always [`SampleFormat::S16le`]: it is what the `PipeWire` tap is
/// configured to emit (guest-image `nixos.nix`) and the only variant v1
/// defines.
pub struct StreamFormat {
    pub rate: u32,
    pub channels: u16,
}

/// PCM-tap reconnect backoff. Starts low because the common case is the
/// daemon racing `pipewire.service` at boot; capped so a long `PipeWire` outage
/// does not turn into a tight loop.
const PCM_BACKOFF_START: Duration = Duration::from_millis(250);
const PCM_BACKOFF_MAX: Duration = Duration::from_secs(5);

/// Tap read buffer: ~10 ms of 48 kHz s16le stereo. Small enough that a chunk
/// is forwarded as soon as the tap produces a quantum (never waiting to fill
/// the buffer: `read` returns whatever is available), large enough that the
/// per-message framing overhead is noise.
const READ_BUF_BYTES: usize = 4096;

/// Object-safe stream, mirroring the compositor's transport: the accept-side
/// types only share Read/Write, and the watchdog thread needs its own handle
/// plus a shutdown lever that reaches the shared socket (dropping one dup'd
/// fd does not unblock a reader parked on another).
trait Conn: Read + Write + Send {
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>>;
    fn shutdown_conn(&self);
}

impl Conn for TcpStream {
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn shutdown_conn(&self) {
        let _ = self.shutdown(std::net::Shutdown::Both);
    }
}

impl Conn for UnixStream {
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn shutdown_conn(&self) {
        let _ = self.shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(target_os = "linux")]
impl Conn for VsockStream {
    fn try_clone_conn(&self) -> std::io::Result<Box<dyn Conn>> {
        Ok(Box::new(self.try_clone()?))
    }
    fn shutdown_conn(&self) {
        let _ = self.shutdown(std::net::Shutdown::Both);
    }
}

enum Acceptor {
    #[cfg(target_os = "linux")]
    Vsock(VsockListener),
    Unix(UnixListener),
    Tcp(TcpListener),
}

/// The Linux half of [`ListenSpec::Vsock`]'s contract.
#[cfg(target_os = "linux")]
fn bind_vsock(port: u32) -> anyhow::Result<Acceptor> {
    let listener = VsockListener::bind_with_cid_port(VMADDR_CID_ANY, port)
        .with_context(|| format!("bind vsock port {port}"))?;
    info!(port, "listening on vsock");
    Ok(Acceptor::Vsock(listener))
}

/// The non-Linux half: `AF_VSOCK` does not exist here, and pretending would
/// only defer the failure to accept time.
#[cfg(not(target_os = "linux"))]
fn bind_vsock(_port: u32) -> anyhow::Result<Acceptor> {
    anyhow::bail!("AF_VSOCK is Linux-only; use --listen-unix or --listen-tcp on a development host")
}

impl Acceptor {
    fn bind(spec: &ListenSpec) -> anyhow::Result<Self> {
        match spec {
            ListenSpec::Vsock(port) => bind_vsock(*port),
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
            #[cfg(target_os = "linux")]
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

/// Bind (fatal so a misconfigured transport fails loudly under systemd
/// `Restart=on-failure`) and serve host connections forever.
///
/// # Errors
/// Only on bind failure; per-connection and PCM-tap failures are retried.
pub fn run(spec: &ListenSpec, pcm_addr: &str, fmt: &StreamFormat) -> anyhow::Result<()> {
    let acceptor = Acceptor::bind(spec)?;
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
        info!("host connected");
        if let Err(err) = serve_conn(&*conn, pcm_addr, fmt) {
            info!(%err, "host connection ended");
        }
        // Unblocks the watchdog thread parked in read_msg on its dup'd fd;
        // dropping our handle alone would leave it parked forever.
        conn.shutdown_conn();
    }
}

/// Drive one host connection until either side dies. `Ok` means the host hung
/// up cleanly (watchdog saw EOF); `Err` carries the handshake or write
/// failure.
fn serve_conn(conn: &dyn Conn, pcm_addr: &str, fmt: &StreamFormat) -> anyhow::Result<()> {
    let mut writer = BufWriter::new(conn.try_clone_conn().context("clone for writer")?);
    let mut reader = BufReader::new(conn.try_clone_conn().context("clone for reader")?);
    exchange_hellos(&mut reader, &mut writer, fmt)?;

    let host_gone = Arc::new(AtomicBool::new(false));
    {
        let host_gone = Arc::clone(&host_gone);
        std::thread::Builder::new()
            .name("panes-audio-watch".into())
            .spawn(move || watch_host(reader, &host_gone))
            .context("spawn watchdog thread")?;
    }

    let frame_bytes = usize::from(fmt.channels) * SampleFormat::S16le.bytes_per_sample();
    let mut backoff = PCM_BACKOFF_START;
    while !host_gone.load(Ordering::Relaxed) {
        let mut pcm = match TcpStream::connect(pcm_addr) {
            Ok(stream) => stream,
            Err(err) => {
                // The tap is pipewire's listener: absent while the service
                // (re)starts. Keep the host connection and retry; the host
                // plays silence from an empty jitter buffer meanwhile.
                warn!(%err, pcm_addr, "PCM tap unreachable; retrying");
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(PCM_BACKOFF_MAX);
                continue;
            }
        };
        backoff = PCM_BACKOFF_START;
        info!(pcm_addr, "PCM tap connected");
        match pump(&mut pcm, &mut writer, frame_bytes, &host_gone) {
            // Tap ended (pipewire restart): reconnect, host stays.
            Ok(PumpEnd::PcmEnded) => debug!("PCM tap ended; reconnecting"),
            Ok(PumpEnd::HostGone) => break,
            Err(err) => return Err(err).context("write to host"),
        }
    }
    Ok(())
}

/// Send our Hello, then read and validate the host's (protocol rule: both
/// sides send immediately on connect, so reading second cannot deadlock).
fn exchange_hellos(
    read: &mut impl Read,
    write: &mut impl Write,
    fmt: &StreamFormat,
) -> anyhow::Result<()> {
    write_msg(
        write,
        &ToHost::Hello {
            major: audio::VERSION_MAJOR,
            minor: audio::VERSION_MINOR,
            rate: fmt.rate,
            channels: fmt.channels,
            format: SampleFormat::S16le,
        },
    )
    .context("send hello")?;
    write.flush().context("flush hello")?;
    let hello: ToGuest = read_msg_bounded(read, audio::MAX_FRAME).context("read host hello")?;
    let ToGuest::Hello { major, minor } = hello;
    anyhow::ensure!(
        major == audio::VERSION_MAJOR,
        "host audio protocol major {major} != {}, hanging up",
        audio::VERSION_MAJOR
    );
    info!(major, minor, "host speaks audio protocol");
    Ok(())
}

/// Park in `read_msg` until the host hangs up (or sends garbage), then raise
/// the flag the pump polls. v1.0 defines no post-Hello host->guest messages
/// and the host gates future ones on our advertised minor, so anything that
/// still decodes is ignored at debug level rather than treated as fatal.
fn watch_host(mut reader: impl Read, host_gone: &AtomicBool) {
    loop {
        match read_msg_bounded::<ToGuest>(&mut reader, audio::MAX_FRAME) {
            Ok(msg) => debug!(?msg, "ignoring unexpected host message"),
            Err(err) => {
                debug!(%err, "host read ended");
                host_gone.store(true, Ordering::Relaxed);
                return;
            }
        }
    }
}

/// Why [`pump`] stopped without a host-side error.
enum PumpEnd {
    /// Tap EOF or read error: reconnect the tap, keep the host connection.
    PcmEnded,
    /// The watchdog flagged the host connection dead.
    HostGone,
}

/// Copy raw PCM from `pcm` into framed [`ToHost::Pcm`] messages on `sink`
/// until one side ends, forwarding each read immediately (no batching beyond
/// what the kernel returned: latency beats throughput here, and flushing per
/// chunk keeps the jitter buffer fed at the tap's own cadence).
///
/// The wire contract requires whole interleaved sample frames per message, so
/// a read that splits a frame keeps the remainder as carry for the next read.
/// On [`PumpEnd::PcmEnded`] any final sub-frame carry is dropped: a torn tap
/// connection loses under one sample frame of audio.
///
/// # Errors
/// Only for sink (host connection) failures; `pcm` errors and EOF are the
/// [`PumpEnd::PcmEnded`] reconnect signal, logged by the caller.
fn pump(
    pcm: &mut impl Read,
    sink: &mut impl Write,
    frame_bytes: usize,
    stop: &AtomicBool,
) -> Result<PumpEnd, WireError> {
    let mut buf = vec![0u8; READ_BUF_BYTES];
    let mut carry: Vec<u8> = Vec::with_capacity(frame_bytes);
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(PumpEnd::HostGone);
        }
        let n = match pcm.read(&mut buf) {
            Ok(0) => return Ok(PumpEnd::PcmEnded),
            Ok(n) => n,
            Err(err) => {
                debug!(%err, "PCM tap read failed");
                return Ok(PumpEnd::PcmEnded);
            }
        };
        carry.extend_from_slice(&buf[..n]);
        let whole = (carry.len() / frame_bytes) * frame_bytes;
        if whole == 0 {
            continue;
        }
        let rest = carry.split_off(whole);
        let payload = std::mem::replace(&mut carry, rest);
        write_msg(sink, &ToHost::Pcm { payload })?;
        sink.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reader that returns its script one slice at a time, then EOF; slice
    /// sizes are deliberately not frame-aligned to exercise the carry.
    struct Scripted {
        chunks: Vec<Vec<u8>>,
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.chunks.is_empty() {
                return Ok(0);
            }
            let chunk = self.chunks.remove(0);
            buf[..chunk.len()].copy_from_slice(&chunk);
            Ok(chunk.len())
        }
    }

    fn decode_payloads(mut wire: &[u8]) -> Vec<Vec<u8>> {
        let mut payloads = Vec::new();
        while !wire.is_empty() {
            let msg: ToHost = read_msg_bounded(&mut wire, audio::MAX_FRAME).unwrap();
            let ToHost::Pcm { payload } = msg else {
                panic!("expected Pcm");
            };
            payloads.push(payload);
        }
        payloads
    }

    #[test]
    fn pump_keeps_frames_whole_across_unaligned_reads() {
        // 11 bytes in 3+3+3+2 chunks with 4-byte frames: the pump must emit
        // 8 bytes (two whole frames) in original order and drop the 3-byte
        // tail as sub-frame carry at EOF.
        let source: Vec<u8> = (0..11).collect();
        let mut pcm = Scripted {
            chunks: vec![
                source[0..3].to_vec(),
                source[3..6].to_vec(),
                source[6..9].to_vec(),
                source[9..11].to_vec(),
            ],
        };
        let mut wire = Vec::new();
        let stop = AtomicBool::new(false);
        let end = pump(&mut pcm, &mut wire, 4, &stop).unwrap();
        assert!(matches!(end, PumpEnd::PcmEnded));
        let bytes: Vec<u8> = decode_payloads(&wire).concat();
        assert_eq!(bytes, source[0..8]);
        for payload in decode_payloads(&wire) {
            assert_eq!(payload.len() % 4, 0, "every message is whole frames");
        }
    }

    #[test]
    fn pump_stops_before_reading_when_host_gone() {
        let mut pcm = Scripted { chunks: vec![vec![0u8; 4]] };
        let mut wire = Vec::new();
        let stop = AtomicBool::new(true);
        let end = pump(&mut pcm, &mut wire, 4, &stop).unwrap();
        assert!(matches!(end, PumpEnd::HostGone));
        assert!(wire.is_empty(), "nothing may be sent after the host died");
    }

    #[test]
    fn hellos_exchange_and_validate_major() {
        let mut host_to_guest = Vec::new();
        write_msg(&mut host_to_guest, &ToGuest::Hello { major: audio::VERSION_MAJOR, minor: 0 })
            .unwrap();
        let mut sent = Vec::new();
        exchange_hellos(
            &mut host_to_guest.as_slice(),
            &mut sent,
            &StreamFormat { rate: 48000, channels: 2 },
        )
        .unwrap();
        let hello: ToHost = read_msg_bounded(&mut sent.as_slice(), audio::MAX_FRAME).unwrap();
        let ToHost::Hello { rate: 48000, channels: 2, format: SampleFormat::S16le, .. } = hello
        else {
            panic!("wrong hello");
        };
    }

    #[test]
    fn hellos_reject_mismatched_major() {
        let mut host_to_guest = Vec::new();
        write_msg(&mut host_to_guest, &ToGuest::Hello { major: audio::VERSION_MAJOR + 1, minor: 0 })
            .unwrap();
        let mut sent = Vec::new();
        let err = exchange_hellos(
            &mut host_to_guest.as_slice(),
            &mut sent,
            &StreamFormat { rate: 48000, channels: 2 },
        )
        .unwrap_err();
        assert!(err.to_string().contains("major"));
    }
}
