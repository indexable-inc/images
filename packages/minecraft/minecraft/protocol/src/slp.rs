//! Server List Ping (SLP): the status handshake every Java Edition server
//! answers on its game port. The multiplayer screen's server list speaks it,
//! and so do this repo's health checks (mc-probe).
//!
//! One exchange: handshake (next state = status) → status request → status
//! response carrying a JSON document → ping/pong round for latency.

use std::io::{self, Read};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::varint::{read_frame, read_varint, write_frame, write_varint};

/// Cap on the status-response frame. Vanilla status JSON with a favicon tops
/// out well under 64 KiB; anything past 1 MiB is a misbehaving peer.
const MAX_STATUS_FRAME: usize = 1 << 20;

/// Protocol version sent in the status handshake. `-1` is the conventional
/// "I'm only asking" value; servers answer the status query regardless.
const STATUS_PROTOCOL_VERSION: i32 = -1;

/// Arbitrary payload for the ping/pong round; the server must echo it back.
const PING_NONCE: i64 = 0x6D63_7072_6F62_6531; // b"mcprobe1"

/// A `host:port` pair for a Java Edition server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerAddress {
    pub host: String,
    pub port: u16,
}

impl ServerAddress {
    /// Parses `host[:port]`, defaulting to the standard Minecraft port 25565.
    /// Bare IPv6 addresses need brackets to carry a port (`[::1]:25565`).
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidInput`] on an empty host or unparseable port.
    pub fn parse(address: &str) -> io::Result<Self> {
        let (host, port) = split_host_port(address)?;
        if host.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("no host in address {address:?}"),
            ));
        }
        Ok(Self {
            host: host.to_owned(),
            port,
        })
    }
}

fn split_host_port(address: &str) -> io::Result<(&str, u16)> {
    let parse_port = |raw: &str| {
        raw.parse::<u16>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid port {raw:?} in address {address:?}"),
            )
        })
    };
    // `[v6]:port` first, so the colons inside the brackets stay untouched.
    if let Some(rest) = address.strip_prefix('[') {
        let (host, after) = rest.split_once(']').ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unclosed '[' in address {address:?}"),
            )
        })?;
        let port = match after.strip_prefix(':') {
            Some(raw) => parse_port(raw)?,
            None if after.is_empty() => DEFAULT_PORT,
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("garbage after ']' in address {address:?}"),
                ));
            }
        };
        return Ok((host, port));
    }
    match address.rsplit_once(':') {
        // More than one colon without brackets is a bare IPv6 address.
        Some((host, _)) if host.contains(':') => Ok((address, DEFAULT_PORT)),
        Some((host, raw_port)) => Ok((host, parse_port(raw_port)?)),
        None => Ok((address, DEFAULT_PORT)),
    }
}

/// The standard Java Edition server port.
pub const DEFAULT_PORT: u16 = 25565;

/// A parsed status response plus the measured ping round-trip.
#[derive(Debug, Clone)]
pub struct SlpStatus {
    /// Display name of the server version, e.g. `"1.21.11"`.
    pub version_name: String,
    /// Numeric protocol version, e.g. 775 for Minecraft 26.1.2.
    pub protocol_version: i32,
    pub players_online: i64,
    pub players_max: i64,
    /// MOTD flattened to text: chat-component trees are concatenated, legacy
    /// format codes are kept verbatim (strip with
    /// [`crate::text::strip_format_codes`]).
    pub motd: String,
    /// The full status JSON, for consumers needing fields beyond the basics.
    pub raw_json: String,
    /// Round-trip time of the ping/pong packet pair.
    pub latency: Duration,
}

impl SlpStatus {
    /// Parses the status-response JSON document.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::InvalidData`] when the document is not JSON or lacks
    /// the mandatory `version`/`players` objects.
    pub fn from_status_json(raw: &str, latency: Duration) -> io::Result<Self> {
        let malformed = |what: &str| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("status response {what}"),
            )
        };
        let root: Value =
            serde_json::from_str(raw).map_err(|err| malformed(&format!("is not JSON: {err}")))?;
        let str_at = |pointer: &str| {
            root.pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| malformed(&format!("lacks string field {pointer}")))
        };
        let int_at = |pointer: &str| {
            root.pointer(pointer)
                .and_then(Value::as_i64)
                .ok_or_else(|| malformed(&format!("lacks integer field {pointer}")))
        };
        let protocol_version = i32::try_from(int_at("/version/protocol")?)
            .map_err(|_| malformed("has an out-of-range /version/protocol"))?;
        let mut motd = String::new();
        if let Some(description) = root.get("description") {
            flatten_component(description, &mut motd);
        }
        Ok(Self {
            version_name: str_at("/version/name")?,
            protocol_version,
            players_online: int_at("/players/online")?,
            players_max: int_at("/players/max")?,
            motd,
            raw_json: raw.to_owned(),
            latency,
        })
    }
}

/// Flattens a chat component (string, array, or object with `text`/`extra`)
/// into its concatenated plain text, ignoring styling.
fn flatten_component(component: &Value, out: &mut String) {
    match component {
        Value::String(text) => out.push_str(text),
        Value::Array(items) => {
            for item in items {
                flatten_component(item, out);
            }
        }
        Value::Object(map) => {
            if let Some(Value::String(text)) = map.get("text") {
                out.push_str(text);
            }
            if let Some(extra) = map.get("extra") {
                flatten_component(extra, out);
            }
        }
        _ => {}
    }
}

/// Performs a full server-list ping: connect, handshake, status request,
/// status response, ping/pong.
///
/// `timeout` bounds the TCP connect and each read/write individually (not
/// the exchange as a whole; DNS resolution is bounded by the OS resolver).
///
/// # Errors
///
/// Connect/read/write failures pass through; a response that is not a valid
/// status document surfaces as [`io::ErrorKind::InvalidData`].
pub fn query(address: &ServerAddress, timeout: Duration) -> io::Result<SlpStatus> {
    let mut stream = connect(address, timeout)?;
    write_frame(&mut stream, &handshake_payload(address))?;
    write_frame(&mut stream, &[0x00])?; // status request: empty body
    let raw_json = read_status_response(&mut stream)?;

    let sent = Instant::now();
    let mut ping = Vec::new();
    write_varint(&mut ping, 0x01);
    ping.extend_from_slice(&PING_NONCE.to_be_bytes());
    write_frame(&mut stream, &ping)?;
    let echoed = read_frame(&mut stream, ping.len())?;
    let latency = sent.elapsed();
    if echoed != ping {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "pong did not echo the ping payload",
        ));
    }

    SlpStatus::from_status_json(&raw_json, latency)
}

fn connect(address: &ServerAddress, timeout: Duration) -> io::Result<TcpStream> {
    let resolved: Vec<SocketAddr> =
        (address.host.as_str(), address.port).to_socket_addrs()?.collect();
    let mut last_error = None;
    for socket_addr in resolved {
        match TcpStream::connect_timeout(&socket_addr, timeout) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout))?;
                stream.set_write_timeout(Some(timeout))?;
                return Ok(stream);
            }
            Err(err) => last_error = Some(err),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("{}:{} resolved to no addresses", address.host, address.port),
        )
    }))
}

fn handshake_payload(address: &ServerAddress) -> Vec<u8> {
    let mut payload = Vec::new();
    write_varint(&mut payload, 0x00); // packet id: handshake
    write_varint(&mut payload, STATUS_PROTOCOL_VERSION);
    write_string(&mut payload, &address.host);
    payload.extend_from_slice(&address.port.to_be_bytes());
    write_varint(&mut payload, 1); // next state: status
    payload
}

fn write_string(out: &mut Vec<u8>, value: &str) {
    // Only ever called with hostnames, which sit far below i32::MAX bytes.
    let length = i32::try_from(value.len()).expect("string length exceeds i32::MAX");
    write_varint(out, length);
    out.extend_from_slice(value.as_bytes());
}

fn read_status_response(stream: &mut TcpStream) -> io::Result<String> {
    let response = read_frame(stream, MAX_STATUS_FRAME)?;
    let mut cursor = response.as_slice();
    let packet_id = read_varint(&mut cursor)?;
    if packet_id != 0x00 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("expected status response packet 0x00, got {packet_id:#x}"),
        ));
    }
    let declared = read_varint(&mut cursor)?;
    let length = usize::try_from(declared).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidData, "negative status JSON length")
    })?;
    let mut bytes = vec![0u8; length];
    cursor.read_exact(&mut bytes)?;
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "status JSON is not UTF-8"))
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[test]
    fn parses_host_only() {
        let addr = ServerAddress::parse("mc.example.net").expect("parse");
        assert_eq!(addr.host, "mc.example.net");
        assert_eq!(addr.port, DEFAULT_PORT);
    }

    #[test]
    fn parses_host_and_port() {
        let addr = ServerAddress::parse("localhost:25566").expect("parse");
        assert_eq!(addr.host, "localhost");
        assert_eq!(addr.port, 25566);
    }

    #[test]
    fn parses_bracketed_ipv6() {
        let addr = ServerAddress::parse("[::1]:25565").expect("parse");
        assert_eq!(addr.host, "::1");
        assert_eq!(addr.port, 25565);
        let bare = ServerAddress::parse("[2001:db8::1]").expect("parse");
        assert_eq!(bare.host, "2001:db8::1");
        assert_eq!(bare.port, DEFAULT_PORT);
    }

    #[test]
    fn parses_bare_ipv6_without_port() {
        let addr = ServerAddress::parse("2001:db8::1").expect("parse");
        assert_eq!(addr.host, "2001:db8::1");
        assert_eq!(addr.port, DEFAULT_PORT);
    }

    #[test]
    fn rejects_bad_addresses() {
        for bad in ["", ":25565", "host:notaport", "host:99999", "[::1", "[::1]x"] {
            let err = ServerAddress::parse(bad).expect_err(bad);
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "{bad}");
        }
    }

    const VANILLA_STATUS: &str = r#"{
        "version": {"name": "26.1.2", "protocol": 775},
        "players": {"online": 3, "max": 20},
        "description": "§6Spleef§r arena"
    }"#;

    #[test]
    fn parses_vanilla_status() {
        let status =
            SlpStatus::from_status_json(VANILLA_STATUS, Duration::ZERO).expect("parse");
        assert_eq!(status.version_name, "26.1.2");
        assert_eq!(status.protocol_version, 775);
        assert_eq!(status.players_online, 3);
        assert_eq!(status.players_max, 20);
        assert_eq!(status.motd, "\u{a7}6Spleef\u{a7}r arena");
        assert_eq!(status.raw_json, VANILLA_STATUS);
    }

    #[test]
    fn flattens_component_motd() {
        let raw = r#"{
            "version": {"name": "x", "protocol": 1},
            "players": {"online": 0, "max": 1},
            "description": {"text": "Hello ", "extra": [{"text": "World"}, "!"]}
        }"#;
        let status = SlpStatus::from_status_json(raw, Duration::ZERO).expect("parse");
        assert_eq!(status.motd, "Hello World!");
    }

    #[test]
    fn missing_description_is_empty_motd() {
        let raw = r#"{"version": {"name": "x", "protocol": 1}, "players": {"online": 0, "max": 1}}"#;
        let status = SlpStatus::from_status_json(raw, Duration::ZERO).expect("parse");
        assert_eq!(status.motd, "");
    }

    #[test]
    fn rejects_malformed_status() {
        for bad in [
            "not json",
            r#"{"players": {"online": 0, "max": 1}}"#,
            r#"{"version": {"name": "x", "protocol": 1}}"#,
            r#"{"version": {"name": "x", "protocol": 99999999999}, "players": {"online": 0, "max": 1}}"#,
        ] {
            let err = SlpStatus::from_status_json(bad, Duration::ZERO).expect_err(bad);
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{bad}");
        }
    }

    /// Serves one SLP exchange on a loopback listener, using the crate's own
    /// framing helpers for the server side.
    fn serve_one_status(listener: &TcpListener, status_json: &str) -> io::Result<()> {
        let (mut stream, _) = listener.accept()?;

        let handshake = read_frame(&mut stream, MAX_STATUS_FRAME)?;
        let mut cursor = handshake.as_slice();
        assert_eq!(read_varint(&mut cursor)?, 0x00, "handshake packet id");
        assert_eq!(read_varint(&mut cursor)?, STATUS_PROTOCOL_VERSION);

        let request = read_frame(&mut stream, MAX_STATUS_FRAME)?;
        assert_eq!(request, [0x00], "status request");

        let mut response = Vec::new();
        write_varint(&mut response, 0x00);
        let json_len = i32::try_from(status_json.len()).expect("test JSON fits in i32");
        write_varint(&mut response, json_len);
        response.extend_from_slice(status_json.as_bytes());
        write_frame(&mut stream, &response)?;

        let ping = read_frame(&mut stream, MAX_STATUS_FRAME)?;
        write_frame(&mut stream, &ping)?; // pong: echo verbatim
        Ok(())
    }

    #[test]
    fn queries_loopback_server() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        let server = thread::spawn(move || serve_one_status(&listener, VANILLA_STATUS));

        let address = ServerAddress {
            host: "127.0.0.1".to_owned(),
            port,
        };
        let status = query(&address, Duration::from_secs(5)).expect("query");
        server.join().expect("server thread").expect("server io");

        assert_eq!(status.version_name, "26.1.2");
        assert_eq!(status.protocol_version, 775);
        assert_eq!(status.players_online, 3);
        assert_eq!(status.players_max, 20);
        assert_eq!(
            crate::text::strip_format_codes(&status.motd),
            "Spleef arena"
        );
    }
}
