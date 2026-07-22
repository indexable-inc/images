//! The recording forward proxy: CONNECT tunnels and absolute-form plain-HTTP
//! requests, forwarded untouched, one [`Connection`] recorded per accepted
//! socket at the moment it closes.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use color_eyre::eyre::{Context, Result, bail, eyre};
use serde::{Deserialize, Serialize};

/// One proxied TCP connection, timed relative to the phase start.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    /// Offset from phase start to the client reaching the proxy.
    pub start_ms: u64,
    /// First contact to both directions closed (or connect failure).
    pub dur_ms: u64,
    pub host: String,
    pub port: u16,
    pub scheme: Scheme,
    /// Bytes the client sent upstream.
    pub bytes_up: u64,
    /// Bytes the upstream sent back.
    pub bytes_down: u64,
    /// The upstream connection failed (unresolvable, refused, timed out).
    /// Failed attempts are still recorded: a blocked fetch is a fetch.
    pub failed: bool,
    /// Closed and fully accounted. Never serialized: snapshots taken while a
    /// tunnel is still open report its duration so far with zero bytes (the
    /// counters live in the copy threads until close).
    #[serde(skip)]
    pub finished: bool,
}

/// How the client asked for the upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    /// `CONNECT host:port` tunnel; TLS stays opaque.
    Connect,
    /// Absolute-form plain-HTTP request.
    Http,
}

/// Collects [`Connection`]s from handler threads. `epoch` anchors every
/// record's `start_ms`, so one recorder spans exactly one phase.
pub struct Recorder {
    epoch: Instant,
    connections: Mutex<Vec<Connection>>,
}

impl Recorder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            connections: Mutex::new(Vec::new()),
        }
    }

    /// Copy of everything recorded so far. Connections registered at accept
    /// time and still open (a keep-alive socket whose upstream has not closed
    /// yet) report their duration up to this snapshot, so a long fetch in
    /// flight at child exit is visible rather than silently dropped.
    ///
    /// # Panics
    /// Panics if a handler thread panicked while holding the lock; handlers
    /// do not panic between lock and unlock.
    #[must_use]
    pub fn snapshot(&self) -> Vec<Connection> {
        let now = self.elapsed_ms();
        let mut connections = self.connections.lock().expect("recorder lock poisoned").clone();
        for connection in &mut connections {
            if !connection.finished {
                connection.dur_ms = now.saturating_sub(connection.start_ms);
            }
        }
        connections
    }

    /// Register a connection at accept time; the handle updates it at close.
    fn begin(&self, connection: Connection) -> usize {
        let mut connections = self.connections.lock().expect("recorder lock poisoned");
        connections.push(connection);
        connections.len() - 1
    }

    fn finish(&self, index: usize, dur_ms: u64, transfer: &Transfer, failed: bool) {
        let mut connections = self.connections.lock().expect("recorder lock poisoned");
        let connection = &mut connections[index];
        connection.dur_ms = dur_ms;
        connection.bytes_up = transfer.up;
        connection.bytes_down = transfer.down;
        connection.failed = failed;
        connection.finished = true;
        // Explicit: release the recorder before any tail work the compiler
        // might add here later (significant_drop_tightening).
        drop(connections);
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).expect("elapsed millis fit u64")
    }
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

/// Budget for reading the request head and reaching the upstream. Tunnels get
/// no idle timeout after establishment: a child's long-lived keep-alive
/// socket closes when the child exits, which is what ends the copy loops.
const IO_TIMEOUT: Duration = Duration::from_mins(1);
/// Request heads beyond this are hostile or corrupt, not HTTP.
const MAX_HEAD: usize = 64 * 1024;

/// Start the proxy on an ephemeral localhost port and return that port.
/// Handler threads live for the life of the process; `run` snapshots the
/// recorder after the child exits rather than joining them.
///
/// # Errors
/// Fails only if the localhost listener cannot bind.
pub fn spawn(recorder: Arc<Recorder>) -> Result<u16> {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).wrap_err("bind localhost proxy listener")?;
    let port = listener.local_addr().wrap_err("read proxy listener port")?.port();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let recorder = Arc::clone(&recorder);
            thread::spawn(move || {
                if let Err(error) = handle(stream, &recorder) {
                    eprintln!("net-trace: connection dropped: {error:#}");
                }
            });
        }
    });
    Ok(port)
}

/// What the request head resolved to.
struct Target {
    host: String,
    port: u16,
    scheme: Scheme,
}

/// Byte counts moved through an established tunnel.
struct Transfer {
    up: u64,
    down: u64,
}

fn handle(mut client: TcpStream, recorder: &Recorder) -> Result<()> {
    let start_ms = recorder.elapsed_ms();
    let started = Instant::now();
    client
        .set_read_timeout(Some(IO_TIMEOUT))
        .wrap_err("set client head-read timeout")?;
    let head = read_head(&mut client)?;
    let target = parse_target(&head.bytes[..head.len])?;

    let index = recorder.begin(Connection {
        start_ms,
        dur_ms: 0,
        host: target.host.clone(),
        port: target.port,
        scheme: target.scheme,
        bytes_up: 0,
        bytes_down: 0,
        failed: false,
        finished: false,
    });
    let record = |transfer: &Transfer, failed: bool| {
        recorder.finish(
            index,
            u64::try_from(started.elapsed().as_millis()).expect("elapsed millis fit u64"),
            transfer,
            failed,
        );
    };

    let upstream = match connect_upstream(&target) {
        Ok(upstream) => upstream,
        Err(error) => {
            record(&Transfer { up: 0, down: 0 }, true);
            // Best effort: the client may already be gone.
            let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
            return Err(error);
        }
    };

    // Head consumed for CONNECT (the tunnel starts fresh after our 200);
    // replayed verbatim for plain HTTP (the upstream needs the request).
    let initial: &[u8] = match target.scheme {
        Scheme::Connect => {
            client
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .wrap_err("acknowledge CONNECT")?;
            &head.bytes[head.len..]
        }
        Scheme::Http => &head.bytes,
    };
    // Long transfers may idle past any sane timeout; the child's exit is the
    // real deadline (its socket EOF ends both copy directions).
    client.set_read_timeout(None).wrap_err("clear tunnel timeout")?;
    let transfer = pump(client, upstream, initial)?;
    record(&transfer, false);
    Ok(())
}

/// The request head plus any bytes read past it (`bytes[len..]`), which
/// belong to the body or tunnel.
struct Head {
    bytes: Vec<u8>,
    len: usize,
}

fn read_head(client: &mut TcpStream) -> Result<Head> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let n = client.read(&mut chunk).wrap_err("read request head")?;
        if n == 0 {
            bail!("client closed before completing request head");
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(end) = find_head_end(&bytes) {
            return Ok(Head { bytes, len: end });
        }
        if bytes.len() > MAX_HEAD {
            bail!("request head exceeds {MAX_HEAD} bytes");
        }
    }
}

fn find_head_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n").map(|i| i + 4)
}

fn parse_target(head: &[u8]) -> Result<Target> {
    let text = std::str::from_utf8(head).wrap_err("request head is not UTF-8")?;
    let request_line = text.lines().next().ok_or_else(|| eyre!("empty request head"))?;
    let mut words = request_line.split_whitespace();
    let method = words.next().ok_or_else(|| eyre!("missing method"))?;
    let uri = words.next().ok_or_else(|| eyre!("missing request target"))?;
    if method == "CONNECT" {
        let authority = Authority::parse(uri, 443)?;
        return Ok(Target {
            host: authority.host,
            port: authority.port,
            scheme: Scheme::Connect,
        });
    }
    let rest = uri
        .strip_prefix("http://")
        .ok_or_else(|| eyre!("non-CONNECT request target must be absolute http://: {uri}"))?;
    let authority_text = rest.split('/').next().unwrap_or(rest);
    let authority = Authority::parse(authority_text, 80)?;
    Ok(Target {
        host: authority.host,
        port: authority.port,
        scheme: Scheme::Http,
    })
}

/// `host[:port]` split out of a request target.
struct Authority {
    host: String,
    port: u16,
}

impl Authority {
    fn parse(text: &str, default_port: u16) -> Result<Self> {
        // IPv6 literals carry colons inside brackets; split on the last colon
        // only when it sits outside the bracketed host.
        let (host, port) = match text.rsplit_once(':') {
            Some((host, port)) if !port.contains(']') => (
                host,
                port.parse::<u16>().wrap_err_with(|| format!("bad port in {text}"))?,
            ),
            _ => (text, default_port),
        };
        if host.is_empty() {
            bail!("empty host in request target {text}");
        }
        Ok(Self {
            host: host.trim_matches(['[', ']']).to_owned(),
            port,
        })
    }
}

/// Try every resolved address, not just the first: a v6-less host whose
/// resolver returns AAAA records first must not turn every fetch into a 502
/// that the untraced gate would have survived.
fn connect_upstream(target: &Target) -> Result<TcpStream> {
    let addresses = (target.host.as_str(), target.port)
        .to_socket_addrs()
        .wrap_err_with(|| format!("resolve {}:{}", target.host, target.port))?;
    let mut last_error = eyre!("no address for {}:{}", target.host, target.port);
    for address in addresses {
        match TcpStream::connect_timeout(&address, IO_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = eyre!("connect {address}: {error}"),
        }
    }
    Err(last_error)
}

fn pump(client: TcpStream, mut upstream: TcpStream, initial_up: &[u8]) -> Result<Transfer> {
    upstream.write_all(initial_up).wrap_err("forward buffered request bytes")?;
    let initial = initial_up.len() as u64;
    let client_reader = client.try_clone().wrap_err("clone client socket")?;
    let upstream_writer = upstream.try_clone().wrap_err("clone upstream socket")?;
    let up_thread = thread::spawn(move || copy_counted(client_reader, upstream_writer));
    let down = copy_counted(upstream, client);
    let up = up_thread.join().map_err(|_| eyre!("upload copy thread panicked"))?;
    Ok(Transfer {
        up: initial + up,
        down,
    })
}

/// Copy until EOF or error, half-closing the write side so the peer's copy
/// direction also ends. Returns bytes moved; errors are indistinguishable
/// from EOF by design (a reset tunnel still gets its record).
fn copy_counted(mut from: TcpStream, mut to: TcpStream) -> u64 {
    let mut buffer = [0_u8; 16 * 1024];
    let mut total = 0_u64;
    loop {
        let n = match from.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if to.write_all(&buffer[..n]).is_err() {
            break;
        }
        total += n as u64;
    }
    let _ = to.shutdown(Shutdown::Write);
    let _ = from.shutdown(Shutdown::Read);
    total
}
