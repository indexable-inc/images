//! Audio playback: connect to the guest's audio vsock port (7102, see the
//! protocol crate's `audio` module), decode the PCM stream, and play it on
//! the default `CoreAudio` output through a jitter buffer.
//!
//! Process shape mirrors `conn`: a supervisor thread owns the socket and
//! reconnects with backoff. Unlike the window stream nothing crosses to the
//! main thread -- samples go straight from the reader into the jitter buffer
//! the `CoreAudio` render callback drains, because every queue hop is latency.

use std::io::{BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use panes_protocol::audio::{MAX_FRAME, SampleFormat, ToGuest, ToHost, VERSION_MAJOR, VERSION_MINOR};
use panes_protocol::{read_msg_bounded, write_msg};

use crate::jitter::JitterBuffer;

pub enum Target {
    Unix(PathBuf),
    Tcp(String),
}

const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_MAX: Duration = Duration::from_secs(5);

/// Jitter buffer sizing, in milliseconds of the guest's advertised stream.
///
/// - Target 24 ms: mid-range of the 20-40 ms the design budget allows
///   (index#1686). Below ~20 ms the vsock + `PipeWire` quantum jitter causes
///   audible underruns; above 40 ms the added latency starts to read as lag
///   in a game. Combined with the guest's ~10 ms `PipeWire` quantum and
///   `CoreAudio`'s ~10 ms default output buffer, end-to-end sits under 50 ms.
/// - Max 96 ms (4x target): enough headroom that slow guest-fast clock drift
///   takes minutes to hit the overrun resync (one dropped-oldest
///   discontinuity) instead of tripping it on every scheduling hiccup.
const TARGET_MS: usize = 24;
const MAX_MS: usize = 96;

pub fn spawn(target: Target) {
    std::thread::spawn(move || supervise(&target));
}

fn supervise(target: &Target) -> ! {
    let mut backoff = BACKOFF_START;
    loop {
        // Connect failures stay quiet: they are the steady state while the
        // guest boots, and the window stream's supervisor already logs its
        // own connect errors for the same endpoint lifecycle.
        if let Ok(stream) = connect(target) {
            // A completed connection resets the backoff: the guest side is
            // probably restarting, not gone.
            backoff = BACKOFF_START;
            if let Err(error) = run_connection(stream) {
                eprintln!("panes-host: audio: {error:#}");
            }
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
            // ~10 ms PCM chunks must not sit in Nagle's buffer waiting for a
            // second chunk; that alone would double the transport latency.
            stream.set_nodelay(true)?;
            let read = stream.try_clone()?;
            Ok(Stream { read: Box::new(read), write: Box::new(stream) })
        }
    }
}

/// One connection: handshake, build the output stream from the guest's
/// advertised format, then feed the jitter buffer until the stream ends.
///
/// `Ok(())` is a clean end (guest hung up); `Err` carries protocol
/// violations and `CoreAudio` failures for the supervisor to log before
/// retrying.
fn run_connection(stream: Stream) -> anyhow::Result<()> {
    let mut write = stream.write;
    // Both sides send their Hello immediately on connect (audio protocol
    // rule), so writing first cannot deadlock against the guest doing the
    // same.
    write_msg(&mut write, &ToGuest::Hello { major: VERSION_MAJOR, minor: VERSION_MINOR })
        .context("send hello")?;
    write.flush().context("flush hello")?;

    let mut reader = BufReader::new(stream.read);
    let first: ToHost = read_msg_bounded(&mut reader, MAX_FRAME).context("read guest hello")?;
    let ToHost::Hello { major, minor, rate, channels, format } = first else {
        anyhow::bail!("guest spoke before Hello");
    };
    anyhow::ensure!(major == VERSION_MAJOR, "guest audio protocol major {major} != {VERSION_MAJOR}");
    anyhow::ensure!(channels >= 1, "guest advertised zero channels");
    // Single-variant enum today; matching keeps a future format addition
    // from being silently played as the wrong encoding.
    let SampleFormat::S16le = format;
    eprintln!(
        "panes-host: audio: guest speaks {major}.{minor}, {rate} Hz x{channels} s16le"
    );

    let per_ms = usize::try_from(rate).context("rate")? * usize::from(channels) / 1000;
    let jitter = Arc::new(JitterBuffer::new(TARGET_MS * per_ms, MAX_MS * per_ms));
    // Bind the stream for the connection's lifetime: dropping it stops
    // playback, which is exactly right when the guest goes away (the jitter
    // buffer would only feed it silence).
    let _output = build_output(rate, channels, Arc::clone(&jitter))?;

    let frame_bytes = usize::from(channels) * format.bytes_per_sample();
    loop {
        match read_msg_bounded::<ToHost>(&mut reader, MAX_FRAME) {
            Ok(ToHost::Pcm { payload }) => {
                anyhow::ensure!(
                    payload.len() % frame_bytes == 0,
                    "PCM payload of {} bytes is not whole {frame_bytes}-byte sample frames",
                    payload.len()
                );
                let samples: Vec<i16> = payload
                    .chunks_exact(2)
                    .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
                jitter.push(&samples);
            }
            Ok(ToHost::Hello { .. }) => anyhow::bail!("guest sent a second Hello"),
            Err(error) => {
                let stats = jitter.stats();
                eprintln!(
                    "panes-host: audio: connection ended ({error}); {} underruns, {} samples dropped",
                    stats.underruns, stats.dropped_samples
                );
                return Ok(());
            }
        }
    }
}

/// Open the default output device at the guest's format. `CoreAudio`'s AUHAL
/// converts our client format (rate/channels) to whatever the device runs,
/// so requesting 48 kHz stereo works on a 44.1 kHz or multichannel device;
/// f32 samples because that is the only client format macOS output units
/// accept natively (cpal does no conversion itself).
fn build_output(
    rate: u32,
    channels: u16,
    jitter: Arc<JitterBuffer>,
) -> anyhow::Result<cpal::Stream> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no default audio output device")?;
    let config = cpal::StreamConfig {
        channels,
        // `cpal::SampleRate` is a plain u32 alias in 0.17.
        sample_rate: rate,
        // The device default (~10 ms on macOS): small enough for the latency
        // budget, and not fighting whatever quantum other apps negotiated.
        buffer_size: cpal::BufferSize::Default,
    };
    let stream = device
        .build_output_stream(
            &config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                // Outcomes are counted inside the buffer; no logging here,
                // this closure runs on the realtime render thread.
                let _ = jitter.pop_f32(data);
            },
            // cpal invokes this off the render path, so logging is safe.
            |error| eprintln!("panes-host: audio: output stream error: {error}"),
            None,
        )
        .context("build CoreAudio output stream")?;
    stream.play().context("start CoreAudio output stream")?;
    Ok(stream)
}
