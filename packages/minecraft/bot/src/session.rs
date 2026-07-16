//! The connection state machine: offline login → configuration → play,
//! answering the server's liveness probes and feeding every clientbound
//! packet from `LoginSuccess` onward to the recorder.
//!
//! The bot is deliberately deaf to game content — chunks, entities, and
//! events are recorded verbatim, never interpreted. The only packets it
//! parses are the handful that demand an answer to keep the session alive
//! (keep-alives, pings, the configuration handshake), which is exactly the
//! set that will not change meaning under a protocol bump without the ids in
//! [`crate::packets`] moving too.

use std::io;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use mc_protocol::ServerAddress;
use mc_protocol::varint::read_varint;
use mc_protocol::wire::read_string;

use crate::framing::Framed;
use crate::mcpr::Recorder;
use crate::packets::{self, clientbound, serverbound};

pub struct Config {
    pub username: String,
    pub protocol_version: i32,
    /// How long to keep recording once the play state is reached.
    pub record_for: Duration,
    /// Per-operation network timeout (connect, and each read/write).
    pub timeout: Duration,
}

/// Runs one full session against `address`, recording into `recorder`.
///
/// Returns once `record_for` elapses in the play state (or earlier if the
/// server goes quiet for a whole `timeout` — the recording is complete
/// either way). Everything captured stays in `recorder` even on error, so a
/// failed session still leaves its trace for the caller to write out.
///
/// # Errors
///
/// Network failures, a kick in any state, or a server that wants online-mode
/// encryption (the bot only speaks offline login).
pub fn run(
    address: &ServerAddress,
    config: &Config,
    recorder: &mut Recorder,
) -> anyhow::Result<()> {
    let stream = address
        .connect(config.timeout)
        .with_context(|| format!("connecting to {}:{}", address.host, address.port))?;
    let mut framed = Framed::new(stream);

    framed.send(&packets::handshake(
        config.protocol_version,
        &address.host,
        address.port,
    ))?;
    framed.send(&packets::login_start(
        &config.username,
        packets::offline_profile_id(&config.username),
    ))?;

    login(&mut framed, recorder).context("login state")?;
    configure(&mut framed, recorder).context("configuration state")?;
    play(&mut framed, recorder, config).context("play state")?;
    Ok(())
}

/// A logical packet split into its `VarInt` id and body. Named (rather than
/// a bare tuple) to satisfy the workspace's
/// `clippy::anonymous_tuple_return_type`.
struct SplitPacket<'a> {
    id: i32,
    body: &'a [u8],
}

/// Splits a logical packet into its `VarInt` id and body.
fn split_id(packet: &[u8]) -> io::Result<SplitPacket<'_>> {
    let mut cursor = packet;
    let id = read_varint(&mut cursor)?;
    Ok(SplitPacket { id, body: cursor })
}

/// Drives the login state until `LoginSuccess` is acknowledged. Recording
/// starts here: `ReplayStudio` expects the stream to open in the LOGIN state,
/// with `LoginSuccess` itself marking the switch to configuration.
/// `SetCompression` is transport negotiation, not session content, and is
/// acted on but not recorded — the stored packets are post-compression
/// anyway.
fn login(framed: &mut Framed<TcpStream>, recorder: &mut Recorder) -> anyhow::Result<()> {
    use clientbound::login as cb;
    loop {
        let packet = framed.recv()?;
        let SplitPacket { id, mut body } = split_id(&packet)?;
        match id {
            cb::SET_COMPRESSION => {
                let threshold = read_varint(&mut body)?;
                // A non-positive threshold disables compression; Minestom
                // only sends the packet when it is enabling it.
                if let Ok(threshold) = usize::try_from(threshold) {
                    framed.enable_compression(threshold);
                }
            }
            cb::LOGIN_SUCCESS => {
                recorder.record(&packet);
                framed.send(&packets::login_acknowledged())?;
                return Ok(());
            }
            cb::PLUGIN_REQUEST => {
                let message_id = read_varint(&mut body)?;
                framed.send(&packets::login_plugin_declined(message_id))?;
            }
            cb::ENCRYPTION_REQUEST => {
                bail!("server demands online-mode encryption; mc-bot only speaks offline login")
            }
            cb::DISCONNECT => {
                // The reason is a JSON text component; surface it verbatim.
                let reason = read_string(&mut body, 1 << 16)?;
                bail!("kicked during login: {reason}")
            }
            other => bail!("unexpected login packet {other:#04x}"),
        }
    }
}

/// Drives the configuration state to `FinishConfiguration`. Registry data,
/// tags, feature flags, and the brand plugin message flow through to the
/// recorder untouched.
fn configure(framed: &mut Framed<TcpStream>, recorder: &mut Recorder) -> anyhow::Result<()> {
    use clientbound::config as cb;
    loop {
        let packet = framed.recv()?;
        recorder.record(&packet);
        let SplitPacket { id, body } = split_id(&packet)?;
        match id {
            cb::SELECT_KNOWN_PACKS => framed.send(&packets::select_known_packs_none())?,
            cb::KEEP_ALIVE => {
                framed.send(&packets::packet(serverbound::config::KEEP_ALIVE, body))?;
            }
            cb::PING => framed.send(&packets::packet(serverbound::config::PONG, body))?,
            cb::FINISH_CONFIGURATION => {
                framed.send(&packets::finish_configuration_ack())?;
                return Ok(());
            }
            // The reason is an NBT text component here; not worth parsing.
            cb::DISCONNECT => bail!("kicked during configuration"),
            _ => {}
        }
    }
}

/// Records the play state until the deadline, echoing liveness probes.
fn play(
    framed: &mut Framed<TcpStream>,
    recorder: &mut Recorder,
    config: &Config,
) -> anyhow::Result<()> {
    use clientbound::play as cb;
    let deadline = Instant::now() + config.record_for;
    loop {
        let Some(remaining) = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
        else {
            return Ok(());
        };
        // Wake at the deadline even if the server goes quiet. A timeout can
        // only fire between packets here (the server writes each frame in
        // one piece, so mid-frame starvation means a dead peer, which the
        // full `config.timeout` already guards).
        framed
            .transport()
            .set_read_timeout(Some(remaining.min(config.timeout)))?;
        let packet = match framed.recv() {
            Ok(packet) => packet,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(err) => return Err(err.into()),
        };
        recorder.record(&packet);
        let SplitPacket { id, body } = split_id(&packet)?;
        match id {
            cb::KEEP_ALIVE => framed.send(&packets::packet(serverbound::play::KEEP_ALIVE, body))?,
            cb::PING => framed.send(&packets::packet(serverbound::play::PONG, body))?,
            cb::DISCONNECT => bail!("kicked during play"),
            _ => {}
        }
    }
}
