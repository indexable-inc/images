//! Loopback proxy that owns an API credential so a sandboxed agent never does.
//!
//! Runs as its own uid, the only one that can read the credential file
//! (materialized 0400 by ix's secret machinery). It accepts plain HTTP on
//! 127.0.0.1, drops whatever credential the client sent, injects the real
//! one, and forwards the request to the single configured upstream over
//! TLS. The response is relayed verbatim, byte for byte, until the upstream
//! closes -- no reframing, no decompression, no header rewriting on the way
//! back -- so streaming (SSE) and compressed bodies pass through untouched.
//!
//! The sandboxed agent's entire network world is this listener (the
//! sandboxed-agent module pins its uid to it with nftables); this process's
//! entire upstream world is the `--upstream` host.
//!
//! argv: `--port N --key-file PATH --upstream HOST --header NAME`, wired by
//! `modules/services/sandboxed-agent`, the single source of truth.

use std::error::Error;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use std::{env, fs, io, thread};

/// Client-supplied identity and framing headers are rewritten here, never
/// passed through: the whole point is that the client's credential is a
/// dummy. `transfer-encoding` is stripped because the body is re-framed
/// with `content-length` (a bodiless request parses as length 0). The
/// configured credential header (`--header`) is stripped alongside these.
const STRIPPED_REQUEST_HEADERS: [&str; 5] = [
    "authorization",
    "connection",
    "content-length",
    "host",
    "transfer-encoding",
];

/// Bounds the gap between upstream reads, not the whole response, so long
/// SSE streams survive as long as they keep producing (the same contract
/// the Python predecessor's socket timeout gave).
const UPSTREAM_TIMEOUT: Duration = Duration::from_mins(10);

/// Longest request head (request line plus headers) accepted from a client.
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Longest request body accepted from a client. Guards the allocation, not
/// the API: a hostile client declaring an absurd content-length must get an
/// error, not a proxy-wide abort from an eager multi-gigabyte reservation.
const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

const MAX_HEADERS: usize = 128;

struct Config {
    port: u16,
    key_file: PathBuf,
    upstream: String,
    header: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let config: &'static Config = Box::leak(Box::new(parse_args(&args)?));
    let tls = tls_client_config();

    // Loopback only: this listener is the sandboxed uid's entire network
    // reach, and nothing else on the machine needs it.
    let listener = TcpListener::bind(("127.0.0.1", config.port))?;
    for client in listener.incoming() {
        let Ok(client) = client else { continue };
        let tls = Arc::clone(&tls);
        // Thread per connection: the agent runs tool calls and API turns
        // concurrently.
        thread::spawn(move || {
            // A failed relay has nobody left to tell: client-visible errors
            // already traveled as HTTP responses, and a torn socket is its
            // own notification.
            let _unreported: io::Result<()> = handle(client, config, &tls);
        });
    }
    Ok(())
}

fn parse_args(args: &[String]) -> Result<Config, Box<dyn Error>> {
    let mut port = None;
    let mut key_file = None;
    let mut upstream = None;
    let mut header = None;

    let mut pairs = args.chunks_exact(2);
    if !pairs.remainder().is_empty() {
        return Err("arguments must be --flag value pairs".into());
    }
    for pair in pairs.by_ref() {
        let flag = pair[0].as_str();
        let value = pair[1].as_str();
        match flag {
            "--port" => port = Some(value.parse::<u16>()?),
            "--key-file" => key_file = Some(PathBuf::from(value)),
            "--upstream" => upstream = Some(value.to_owned()),
            "--header" => header = Some(value.to_owned()),
            _ => return Err(format!("unknown argument: {flag}").into()),
        }
    }

    Ok(Config {
        port: port.ok_or("--port is required")?,
        key_file: key_file.ok_or("--key-file is required")?,
        upstream: upstream.ok_or("--upstream is required")?,
        header: header.ok_or("--header is required")?,
    })
}

/// Compiled-in Mozilla roots (webpki-roots) rather than a system bundle:
/// the service runs hardened with no interest in host CA state, and the
/// one upstream it will ever verify carries a public certificate.
fn tls_client_config() -> Arc<rustls::ClientConfig> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

fn handle(
    mut client: TcpStream,
    config: &Config,
    tls: &Arc<rustls::ClientConfig>,
) -> io::Result<()> {
    // SSE tokens must reach the agent as they arrive, not when Nagle's
    // algorithm decides a segment is full.
    client.set_nodelay(true)?;
    // A client that stalls mid-request (or stops draining a response) for
    // this long forfeits its thread; thread-per-connection has no other
    // backpressure.
    client.set_read_timeout(Some(UPSTREAM_TIMEOUT))?;
    client.set_write_timeout(Some(UPSTREAM_TIMEOUT))?;

    let RequestBytes { head, body_prefix } = match read_head(&mut client) {
        Ok(parts) => parts,
        Err(err) => return respond(&mut client, 400, "Bad Request", &err.to_string()),
    };

    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut request = httparse::Request::new(&mut headers);
    match request.parse(&head) {
        // `head` ends at the blank line, so a well-formed request parses
        // completely; anything partial here is malformed.
        Ok(status) if status.is_complete() => {}
        Ok(_) | Err(_) => {
            return respond(&mut client, 400, "Bad Request", "malformed request head");
        }
    }
    let (Some(method), Some(path)) = (request.method, request.path) else {
        return respond(&mut client, 400, "Bad Request", "malformed request line");
    };

    let body = match read_body(&mut client, &request, body_prefix) {
        Ok(body) => body,
        Err(err) => return respond(&mut client, 400, "Bad Request", &err.to_string()),
    };

    // Read the key per request, not at startup: the service comes up on a
    // fresh VM before the secret is attached, and rotation (recreate the
    // VM, or replace the file) needs no proxy restart.
    let key = match read_key(&config.key_file) {
        Ok(key) => key,
        Err(reason) => {
            let detail = format!(
                "credential file {} not usable: {reason}; attach the secret and recreate the VM",
                config.key_file.display(),
            );
            return respond(&mut client, 503, "Service Unavailable", &detail);
        }
    };

    let request_bytes = upstream_request(config, method, path, request.headers, &key, &body);
    match relay(&mut client, config, tls, &request_bytes) {
        Ok(()) => Ok(()),
        Err(RelayError::BeforeResponse(err)) => {
            respond(&mut client, 502, "Bad Gateway", &format!("upstream unreachable: {err}"))
        }
        // Mid-stream failures cannot become a clean HTTP error anymore;
        // closing the client socket is the honest signal left.
        Err(RelayError::MidStream(err)) => Err(err),
    }
}

/// The client's request, split at the end of the head: the head bytes
/// (through the blank line) and whatever body bytes rode the same reads.
struct RequestBytes {
    head: Vec<u8>,
    body_prefix: Vec<u8>,
}

fn read_head(client: &mut TcpStream) -> io::Result<RequestBytes> {
    let mut data = Vec::new();
    let mut buf = [0_u8; 8 * 1024];
    loop {
        if let Some(end) = head_end(&data) {
            let body_prefix = data.split_off(end);
            return Ok(RequestBytes {
                head: data,
                body_prefix,
            });
        }
        if data.len() > MAX_HEAD_BYTES {
            return Err(io::Error::new(ErrorKind::InvalidData, "request head too large"));
        }
        let count = client.read(&mut buf)?;
        if count == 0 {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "client closed before completing the request head",
            ));
        }
        data.extend_from_slice(&buf[..count]);
    }
}

fn head_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n").map(|at| at + 4)
}

/// The body is whatever `content-length` promises; an absent header is a
/// bodiless request, not a chunked one (the Anthropic SDK always sends
/// `content-length`, and `transfer-encoding` is stripped on the way out).
fn read_body(
    client: &mut TcpStream,
    request: &httparse::Request,
    prefix: Vec<u8>,
) -> io::Result<Vec<u8>> {
    let mut length = 0_usize;
    for header in &*request.headers {
        if header.name.eq_ignore_ascii_case("content-length") {
            length = std::str::from_utf8(header.value)
                .ok()
                .and_then(|value| value.trim().parse().ok())
                .ok_or_else(|| {
                    io::Error::new(ErrorKind::InvalidData, "unparseable content-length")
                })?;
        }
    }
    if length > MAX_BODY_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("content-length {length} exceeds the {MAX_BODY_BYTES}-byte limit"),
        ));
    }
    let mut body = prefix;
    // Pipelined bytes beyond the declared body are dropped: every response
    // closes the connection, so there is no next request to serve.
    body.truncate(length);
    // Grow with the bytes that actually arrive; an up-front reservation of
    // the declared length would let one dishonest header exhaust memory.
    let mut buf = vec![0_u8; 64 * 1024];
    while body.len() < length {
        let want = (length - body.len()).min(buf.len());
        let count = client.read(&mut buf[..want])?;
        if count == 0 {
            return Err(io::Error::new(
                ErrorKind::UnexpectedEof,
                "client closed before completing the request body",
            ));
        }
        body.extend_from_slice(&buf[..count]);
    }
    Ok(body)
}

fn read_key(path: &Path) -> Result<String, String> {
    let raw = fs::read_to_string(path).map_err(|err| err.to_string())?;
    let key = raw.trim();
    if key.is_empty() {
        return Err("file is empty".to_owned());
    }
    // A header value must stay a single clean line; anything else would let
    // file contents rewrite the forwarded request.
    if !key.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err("file holds bytes that cannot appear in an HTTP header value".to_owned());
    }
    Ok(key.to_owned())
}

/// Serialize the upstream request: the client's request line and headers
/// verbatim, minus [`STRIPPED_REQUEST_HEADERS`] and the credential header,
/// plus the rewritten host, credential, and framing.
fn upstream_request(
    config: &Config,
    method: &str,
    path: &str,
    headers: &[httparse::Header],
    key: &str,
    body: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(MAX_HEAD_BYTES.min(4 * 1024) + body.len());
    out.extend_from_slice(format!("{method} {path} HTTP/1.1\r\n").as_bytes());
    for header in headers {
        let stripped = STRIPPED_REQUEST_HEADERS
            .iter()
            .any(|name| header.name.eq_ignore_ascii_case(name))
            || header.name.eq_ignore_ascii_case(&config.header);
        if stripped {
            continue;
        }
        out.extend_from_slice(header.name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(header.value);
        out.extend_from_slice(b"\r\n");
    }
    let tail = format!(
        "host: {}\r\n{}: {key}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        config.upstream,
        config.header,
        body.len(),
    );
    out.extend_from_slice(tail.as_bytes());
    out.extend_from_slice(body);
    out
}

enum RelayError {
    /// Failed before any upstream byte reached the client; a clean HTTP
    /// error response is still possible.
    BeforeResponse(io::Error),
    MidStream(io::Error),
}

/// Send the serialized request upstream over TLS and relay the raw response
/// bytes back until the upstream closes. `connection: close` was written
/// into the request, so end-of-response is end-of-stream; the client parses
/// the upstream's own status line, headers, and framing untouched.
fn relay(
    client: &mut TcpStream,
    config: &Config,
    tls: &Arc<rustls::ClientConfig>,
    request_bytes: &[u8],
) -> Result<(), RelayError> {
    let mut tcp = TcpStream::connect((config.upstream.as_str(), 443))
        .map_err(RelayError::BeforeResponse)?;
    tcp.set_read_timeout(Some(UPSTREAM_TIMEOUT))
        .map_err(RelayError::BeforeResponse)?;
    tcp.set_write_timeout(Some(UPSTREAM_TIMEOUT))
        .map_err(RelayError::BeforeResponse)?;
    let name = rustls::pki_types::ServerName::try_from(config.upstream.clone())
        .map_err(|err| RelayError::BeforeResponse(io::Error::new(ErrorKind::InvalidInput, err)))?;
    let mut conn = rustls::ClientConnection::new(Arc::clone(tls), name)
        .map_err(|err| RelayError::BeforeResponse(io::Error::other(err)))?;
    let mut upstream = rustls::Stream::new(&mut conn, &mut tcp);

    upstream
        .write_all(request_bytes)
        .map_err(RelayError::BeforeResponse)?;

    let mut sent_any = false;
    // Heap-allocated: 64 KiB per relay thread is too big for comfort on the
    // 8 KiB-happy default stack budget clippy enforces.
    let mut buf = vec![0_u8; 64 * 1024];
    loop {
        let count = match upstream.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(count) => count,
            // Close without close_notify: treated as end of stream, the
            // way every HTTP client treats a close-delimited response.
            Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(()),
            Err(err) if sent_any => return Err(RelayError::MidStream(err)),
            Err(err) => return Err(RelayError::BeforeResponse(err)),
        };
        client
            .write_all(&buf[..count])
            .map_err(RelayError::MidStream)?;
        sent_any = true;
    }
}

fn respond(client: &mut TcpStream, status: u16, reason: &str, detail: &str) -> io::Result<()> {
    let body = format!("{detail}\n");
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len(),
    );
    client.write_all(head.as_bytes())?;
    client.write_all(body.as_bytes())
}
