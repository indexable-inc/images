use std::{
    collections::BTreeSet,
    io::{ErrorKind, Read, Write},
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use clap::{Parser, ValueEnum};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error as RustlsError, RootCertStore,
    SignatureScheme, StreamOwned,
    client::{
        WebPkiServerVerifier,
        danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    },
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use url::Url;
use x509_parser::{extensions::GeneralName, prelude::*};

const DEFAULT_URL: &str = "https://ix.dev/";
const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_READ_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_MAX_BODY_BYTES: usize = 64 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const RESPONSE_PREFIX_BYTES: usize = 96;

#[derive(Parser)]
#[command(
    about = "Probe ix.dev HTTPS reachability and print JSON diagnostics.",
    version
)]
struct Args {
    /// HTTPS URL to probe.
    #[arg(default_value = DEFAULT_URL)]
    url: String,

    /// Restrict probes to one address family.
    #[arg(long, value_enum, default_value_t = AddressFamily::Any)]
    family: AddressFamily,

    /// TCP connect timeout per address.
    #[arg(long, default_value_t = DEFAULT_CONNECT_TIMEOUT_MS)]
    connect_timeout_ms: u64,

    /// TLS and HTTP read/write timeout per address.
    #[arg(long, default_value_t = DEFAULT_READ_TIMEOUT_MS)]
    read_timeout_ms: u64,

    /// Maximum response body bytes to retain in the JSON report.
    #[arg(long, default_value_t = DEFAULT_MAX_BODY_BYTES)]
    max_body_bytes: usize,

    /// Pretty-print the JSON report.
    #[arg(long)]
    pretty: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum AddressFamily {
    Any,
    Ipv4,
    Ipv6,
}

#[derive(Serialize)]
struct Report {
    command: CommandReport,
    target: TargetReport,
    trust_roots: TrustRootsReport,
    dns: DnsReport,
    probes: Vec<ProbeReport>,
    summary: SummaryReport,
}

#[derive(Serialize)]
struct CommandReport {
    name: &'static str,
    version: &'static str,
    started_unix_ms: u64,
}

#[derive(Serialize)]
struct TargetReport {
    url: String,
    host: String,
    port: u16,
    path_and_query: String,
}

#[derive(Serialize)]
struct TrustRootsReport {
    native: RootStoreReport,
    mozilla: RootStoreReport,
}

#[derive(Serialize)]
struct RootStoreReport {
    loaded: usize,
    accepted: usize,
    ignored: usize,
    errors: Vec<String>,
}

#[derive(Serialize)]
struct DnsReport {
    resolver: &'static str,
    addresses: Vec<AddressReport>,
    error: Option<String>,
}

#[derive(Serialize, Clone)]
struct AddressReport {
    socket_addr: String,
    ip: String,
    family: &'static str,
}

#[derive(Serialize)]
struct ProbeReport {
    address: AddressReport,
    tcp: TcpReport,
    tls: Option<TlsReport>,
    http: Option<HttpReport>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum TcpReport {
    Connected {
        elapsed_ms: u64,
        local_addr: Option<String>,
    },
    Failed {
        elapsed_ms: u64,
        error: String,
    },
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum TlsReport {
    Completed {
        elapsed_ms: u64,
        verification_mode: &'static str,
        protocol_version: Option<String>,
        cipher_suite: Option<String>,
        alpn_protocol: Option<String>,
        certificate_verification: CertificateVerificationReport,
        peer_certificates: Vec<CertificateReport>,
    },
    Failed {
        elapsed_ms: u64,
        error: String,
        certificate_verification: CertificateVerificationReport,
        peer_certificates: Vec<CertificateReport>,
    },
}

#[derive(Serialize)]
struct CertificateVerificationReport {
    native_roots: VerificationStatus,
    mozilla_roots: VerificationStatus,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "status", rename_all = "snake_case")]
enum VerificationStatus {
    Passed,
    Failed { error: String },
    NotRun { reason: String },
}

#[derive(Serialize)]
struct CertificateReport {
    index: usize,
    sha256: String,
    subject: Option<String>,
    issuer: Option<String>,
    serial: Option<String>,
    not_before: Option<String>,
    not_after: Option<String>,
    subject_alt_names: SubjectAltNameReport,
    parse_error: Option<String>,
}

#[derive(Default, Serialize)]
struct SubjectAltNameReport {
    dns_names: Vec<String>,
    ip_addresses: Vec<String>,
    uris: Vec<String>,
}

#[derive(Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum HttpReport {
    Completed {
        elapsed_ms: u64,
        status_code: Option<u16>,
        reason: Option<String>,
        headers: Vec<HttpHeader>,
        header_bytes: usize,
        response_complete: bool,
        read_limit_reached: bool,
        body_sample_bytes: usize,
        body_sample_complete: bool,
        body_sample_sha256: String,
        body_sample_base64: String,
        response_prefix_hex: String,
    },
    Failed {
        elapsed_ms: u64,
        error: String,
        bytes_read: usize,
        response_prefix_hex: String,
    },
}

#[derive(Serialize)]
struct HttpHeader {
    name: String,
    value: String,
}

#[derive(Serialize)]
struct SummaryReport {
    ok: bool,
    diagnoses: Vec<String>,
}

#[derive(Debug, Default, Clone)]
struct VerificationCapture {
    peer_certificates: Vec<Vec<u8>>,
    native_roots: Option<VerificationStatus>,
    mozilla_roots: Option<VerificationStatus>,
}

#[derive(Debug)]
struct RecordingVerifier {
    native: Option<Arc<WebPkiServerVerifier>>,
    mozilla: Arc<WebPkiServerVerifier>,
    capture: Arc<Mutex<VerificationCapture>>,
}

struct RootVerifiers {
    native: Option<Arc<WebPkiServerVerifier>>,
    mozilla: Arc<WebPkiServerVerifier>,
    report: TrustRootsReport,
}

struct TlsOutcome {
    stream: StreamOwned<ClientConnection, TcpStream>,
    report: TlsReport,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let pretty = args.pretty;
    let report = run(&args)?;
    if pretty {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", serde_json::to_string(&report)?);
    }
    Ok(())
}

fn run(args: &Args) -> Result<Report> {
    let started_unix_ms = unix_ms(SystemTime::now());
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    let url = Url::parse(&args.url).with_context(|| format!("failed to parse {}", args.url))?;
    if url.scheme() != "https" {
        bail!("ix-dev-diagnose only supports https URLs");
    }

    let host = url
        .host_str()
        .context("target URL must include a host")?
        .to_owned();
    let port = url.port_or_known_default().unwrap_or(443);
    let path_and_query = path_and_query(&url);
    let trust_roots = build_root_verifiers()?;
    let (addresses, dns_error) = resolve_addresses(&host, port, args.family);
    let address_reports = addresses.iter().copied().map(AddressReport::from).collect();

    let target = TargetReport {
        url: url.to_string(),
        host,
        port,
        path_and_query,
    };
    let probes = addresses
        .iter()
        .copied()
        .map(|address| probe_address(address, &target, &trust_roots, args))
        .collect::<Vec<_>>();
    let dns = DnsReport {
        resolver: "system",
        addresses: address_reports,
        error: dns_error,
    };
    let summary = summarize(&dns, &probes);

    Ok(Report {
        command: CommandReport {
            name: "ix-dev-diagnose",
            version: env!("CARGO_PKG_VERSION"),
            started_unix_ms,
        },
        target,
        trust_roots: trust_roots.report,
        dns,
        probes,
        summary,
    })
}

fn build_root_verifiers() -> Result<RootVerifiers> {
    let native_result = rustls_native_certs::load_native_certs();
    let native_loaded = native_result.certs.len();
    let native_errors = native_result
        .errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let mut native_store = RootCertStore::empty();
    let (native_accepted, native_ignored) =
        native_store.add_parsable_certificates(native_result.certs);
    let native = if native_store.is_empty() {
        None
    } else {
        Some(
            WebPkiServerVerifier::builder(Arc::new(native_store))
                .build()
                .context("failed to build native-root verifier")?,
        )
    };

    let mozilla_loaded = webpki_roots::TLS_SERVER_ROOTS.len();
    let mut mozilla_store = RootCertStore::empty();
    mozilla_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mozilla = WebPkiServerVerifier::builder(Arc::new(mozilla_store))
        .build()
        .context("failed to build Mozilla-root verifier")?;

    Ok(RootVerifiers {
        native,
        mozilla,
        report: TrustRootsReport {
            native: RootStoreReport {
                loaded: native_loaded,
                accepted: native_accepted,
                ignored: native_ignored,
                errors: native_errors,
            },
            mozilla: RootStoreReport {
                loaded: mozilla_loaded,
                accepted: mozilla_loaded,
                ignored: 0,
                errors: Vec::new(),
            },
        },
    })
}

fn resolve_addresses(
    host: &str,
    port: u16,
    family: AddressFamily,
) -> (Vec<SocketAddr>, Option<String>) {
    match (host, port).to_socket_addrs() {
        Ok(addrs) => {
            let addresses = addrs
                .filter(|address| family.matches(address.ip()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            (addresses, None)
        }
        Err(error) => (Vec::new(), Some(error.to_string())),
    }
}

fn probe_address(
    address: SocketAddr,
    target: &TargetReport,
    trust_roots: &RootVerifiers,
    args: &Args,
) -> ProbeReport {
    let address_report = AddressReport::from(address);
    let connect_start = Instant::now();
    let connect_timeout = Duration::from_millis(args.connect_timeout_ms);
    let stream = match TcpStream::connect_timeout(&address, connect_timeout) {
        Ok(stream) => stream,
        Err(error) => {
            return ProbeReport {
                address: address_report,
                tcp: TcpReport::Failed {
                    elapsed_ms: elapsed_ms(connect_start),
                    error: error.to_string(),
                },
                tls: None,
                http: None,
            };
        }
    };

    let local_addr = stream.local_addr().ok().map(|addr| addr.to_string());
    let tcp = TcpReport::Connected {
        elapsed_ms: elapsed_ms(connect_start),
        local_addr,
    };
    let timeout = Duration::from_millis(args.read_timeout_ms);
    if let Err(error) = stream.set_read_timeout(Some(timeout)) {
        return tls_setup_failure(address_report, tcp, &error);
    }
    if let Err(error) = stream.set_write_timeout(Some(timeout)) {
        return tls_setup_failure(address_report, tcp, &error);
    }

    match probe_tls(stream, target, trust_roots) {
        Ok(mut outcome) => {
            let http = Some(probe_http(&mut outcome.stream, target, args.max_body_bytes));
            ProbeReport {
                address: address_report,
                tcp,
                tls: Some(outcome.report),
                http,
            }
        }
        Err(report) => ProbeReport {
            address: address_report,
            tcp,
            tls: Some(*report),
            http: None,
        },
    }
}

fn tls_setup_failure(
    address: AddressReport,
    tcp: TcpReport,
    error: &std::io::Error,
) -> ProbeReport {
    ProbeReport {
        address,
        tcp,
        tls: Some(TlsReport::Failed {
            elapsed_ms: 0,
            error: format!("failed to configure socket timeout: {error}"),
            certificate_verification: CertificateVerificationReport::not_run(
                "TLS handshake did not start",
            ),
            peer_certificates: Vec::new(),
        }),
        http: None,
    }
}

fn probe_tls(
    stream: TcpStream,
    target: &TargetReport,
    trust_roots: &RootVerifiers,
) -> std::result::Result<TlsOutcome, Box<TlsReport>> {
    let capture = Arc::new(Mutex::new(VerificationCapture::default()));
    let verifier = RecordingVerifier {
        native: trust_roots.native.clone(),
        mozilla: Arc::clone(&trust_roots.mozilla),
        capture: Arc::clone(&capture),
    };
    let mut config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    let server_name = match ServerName::try_from(target.host.clone()) {
        Ok(name) => name,
        Err(error) => {
            return Err(Box::new(TlsReport::Failed {
                elapsed_ms: 0,
                error: format!("invalid TLS server name: {error}"),
                certificate_verification: CertificateVerificationReport::not_run(
                    "invalid server name",
                ),
                peer_certificates: Vec::new(),
            }));
        }
    };
    let mut connection = match ClientConnection::new(Arc::new(config), server_name) {
        Ok(connection) => connection,
        Err(error) => {
            return Err(Box::new(TlsReport::Failed {
                elapsed_ms: 0,
                error: error.to_string(),
                certificate_verification: CertificateVerificationReport::not_run(
                    "TLS client setup failed",
                ),
                peer_certificates: Vec::new(),
            }));
        }
    };

    let tls_start = Instant::now();
    let mut socket = stream;
    while connection.is_handshaking() {
        if let Err(error) = connection.complete_io(&mut socket) {
            let captured = captured_verification(&capture);
            return Err(Box::new(TlsReport::Failed {
                elapsed_ms: elapsed_ms(tls_start),
                error: error.to_string(),
                certificate_verification: captured.verification,
                peer_certificates: captured.certificates,
            }));
        }
    }

    let captured = captured_verification(&capture);
    let report = TlsReport::Completed {
        elapsed_ms: elapsed_ms(tls_start),
        verification_mode: "recording_verifier",
        protocol_version: connection
            .protocol_version()
            .map(|version| format!("{version:?}")),
        cipher_suite: connection
            .negotiated_cipher_suite()
            .map(|suite| format!("{:?}", suite.suite())),
        alpn_protocol: connection
            .alpn_protocol()
            .map(|protocol| String::from_utf8_lossy(protocol).into_owned()),
        certificate_verification: captured.verification,
        peer_certificates: captured.certificates,
    };

    Ok(TlsOutcome {
        stream: StreamOwned::new(connection, socket),
        report,
    })
}

fn probe_http(
    stream: &mut StreamOwned<ClientConnection, TcpStream>,
    target: &TargetReport,
    max_body_bytes: usize,
) -> HttpReport {
    let start = Instant::now();
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: ix-dev-diagnose/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        target.path_and_query,
        host_header(&target.host, target.port),
        env!("CARGO_PKG_VERSION"),
    );
    if let Err(error) = stream.write_all(request.as_bytes()) {
        return HttpReport::Failed {
            elapsed_ms: elapsed_ms(start),
            error: error.to_string(),
            bytes_read: 0,
            response_prefix_hex: String::new(),
        };
    }
    if let Err(error) = stream.flush() {
        return HttpReport::Failed {
            elapsed_ms: elapsed_ms(start),
            error: error.to_string(),
            bytes_read: 0,
            response_prefix_hex: String::new(),
        };
    }

    let limit = MAX_HEADER_BYTES.saturating_add(max_body_bytes);
    let mut response = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut response_complete = false;
    let mut read_limit_reached = false;
    loop {
        let remaining = limit.saturating_sub(response.len());
        if remaining == 0 {
            read_limit_reached = true;
            break;
        }
        let read_len = buffer.len().min(remaining);
        match stream.read(&mut buffer[..read_len]) {
            Ok(0) => {
                response_complete = eof_completes_response(&response);
                break;
            }
            Ok(count) => {
                response.extend_from_slice(&buffer[..count]);
                if response_has_complete_framing(&response) {
                    response_complete = true;
                    break;
                }
            }
            Err(error) if error.kind() == ErrorKind::UnexpectedEof && !response.is_empty() => {
                response_complete = eof_completes_response(&response);
                break;
            }
            Err(error) => {
                return HttpReport::Failed {
                    elapsed_ms: elapsed_ms(start),
                    error: error.to_string(),
                    bytes_read: response.len(),
                    response_prefix_hex: hex_prefix(&response),
                };
            }
        }
    }

    parse_http_response(
        &response,
        max_body_bytes,
        response_complete,
        read_limit_reached,
        elapsed_ms(start),
    )
}

fn parse_http_response(
    response: &[u8],
    max_body_bytes: usize,
    response_complete: bool,
    read_limit_reached: bool,
    elapsed_ms: u64,
) -> HttpReport {
    let Some((header_start, header_end)) = final_header_block(response) else {
        return HttpReport::Failed {
            elapsed_ms,
            error: "response did not contain complete HTTP headers".to_owned(),
            bytes_read: response.len(),
            response_prefix_hex: hex_prefix(response),
        };
    };
    let header_bytes = &response[header_start..header_end];
    let body_start = header_end + 4;
    let body = response.get(body_start..).unwrap_or_default();
    let body_sample = &body[..body.len().min(max_body_bytes)];
    let (status_code, reason, headers) = parse_headers(header_bytes);

    HttpReport::Completed {
        elapsed_ms,
        status_code,
        reason,
        headers,
        header_bytes: header_bytes.len(),
        response_complete,
        read_limit_reached,
        body_sample_bytes: body_sample.len(),
        body_sample_complete: response_complete && body.len() <= max_body_bytes,
        body_sample_sha256: sha256_hex(body_sample),
        body_sample_base64: BASE64_STANDARD.encode(body_sample),
        response_prefix_hex: hex_prefix(response),
    }
}

fn parse_headers(header_bytes: &[u8]) -> (Option<u16>, Option<String>, Vec<HttpHeader>) {
    let header_text = String::from_utf8_lossy(header_bytes);
    let mut lines = header_text.split("\r\n");
    let (status_code, reason) = lines.next().map_or((None, None), parse_status_line);
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some(HttpHeader {
                name: name.trim().to_owned(),
                value: value.trim().to_owned(),
            })
        })
        .collect();

    (status_code, reason, headers)
}

fn parse_status_line(line: &str) -> (Option<u16>, Option<String>) {
    let mut parts = line.splitn(3, ' ');
    let _version = parts.next();
    let status_code = parts.next().and_then(|code| code.parse().ok());
    let reason = parts.next().map(str::to_owned);
    (status_code, reason)
}

fn final_header_block(response: &[u8]) -> Option<(usize, usize)> {
    let mut header_start = 0;
    loop {
        let header_end = header_start + find_header_end(&response[header_start..])?;
        let header_bytes = &response[header_start..header_end];
        let (status_code, _reason, _headers) = parse_headers(header_bytes);
        if matches!(status_code, Some(100..=199)) {
            header_start = header_end + 4;
            continue;
        }

        return Some((header_start, header_end));
    }
}

fn response_has_complete_framing(response: &[u8]) -> bool {
    matches!(response_framing(response), ResponseFraming::Complete)
}

fn eof_completes_response(response: &[u8]) -> bool {
    matches!(
        response_framing(response),
        ResponseFraming::Complete | ResponseFraming::CloseDelimited
    )
}

fn response_framing(response: &[u8]) -> ResponseFraming {
    let Some((header_start, header_end)) = final_header_block(response) else {
        return ResponseFraming::Incomplete;
    };
    let header_bytes = &response[header_start..header_end];
    let (status_code, _reason, headers) = parse_headers(header_bytes);
    if matches!(status_code, Some(204 | 205 | 304)) {
        return ResponseFraming::Complete;
    }

    let body_start = header_end + 4;
    let body = response.get(body_start..).unwrap_or_default();
    if transfer_encoding_is_chunked(&headers) {
        return if chunked_body_complete(body) {
            ResponseFraming::Complete
        } else {
            ResponseFraming::Incomplete
        };
    }
    if let Some(content_length) = content_length(&headers) {
        return if body.len() >= content_length {
            ResponseFraming::Complete
        } else {
            ResponseFraming::Incomplete
        };
    }

    ResponseFraming::CloseDelimited
}

#[derive(Clone, Copy)]
enum ResponseFraming {
    Complete,
    Incomplete,
    CloseDelimited,
}

fn content_length(headers: &[HttpHeader]) -> Option<usize> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-length"))
        .and_then(|header| header.value.parse().ok())
}

fn transfer_encoding_is_chunked(headers: &[HttpHeader]) -> bool {
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("transfer-encoding"))
        .flat_map(|header| header.value.split(','))
        .any(|coding| coding.trim().eq_ignore_ascii_case("chunked"))
}

fn chunked_body_complete(mut body: &[u8]) -> bool {
    loop {
        let Some(size_line_end) = find_crlf(body) else {
            return false;
        };
        let size_line = &body[..size_line_end];
        let Some(chunk_size) = parse_chunk_size(size_line) else {
            return false;
        };
        body = &body[size_line_end + 2..];
        if chunk_size == 0 {
            return trailer_block_complete(body);
        }
        let Some(chunk_with_crlf) = chunk_size.checked_add(2) else {
            return false;
        };
        if body.len() < chunk_with_crlf {
            return false;
        }
        if &body[chunk_size..chunk_size + 2] != b"\r\n" {
            return false;
        }
        body = &body[chunk_with_crlf..];
    }
}

fn parse_chunk_size(size_line: &[u8]) -> Option<usize> {
    let size_text = std::str::from_utf8(size_line).ok()?;
    let size_hex = size_text
        .split_once(';')
        .map_or(size_text, |(size, _)| size);
    usize::from_str_radix(size_hex.trim(), 16).ok()
}

fn trailer_block_complete(trailers: &[u8]) -> bool {
    trailers.starts_with(b"\r\n") || find_header_end(trailers).is_some()
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
}

fn captured_verification(capture: &Mutex<VerificationCapture>) -> CapturedVerification {
    let captured = capture.lock().map_or_else(
        |_| VerificationCapture {
            peer_certificates: Vec::new(),
            native_roots: Some(VerificationStatus::NotRun {
                reason: "verification capture mutex was poisoned".to_owned(),
            }),
            mozilla_roots: Some(VerificationStatus::NotRun {
                reason: "verification capture mutex was poisoned".to_owned(),
            }),
        },
        |guard| guard.clone(),
    );
    let certificates = captured
        .peer_certificates
        .iter()
        .enumerate()
        .map(|(index, der)| certificate_report(index, der))
        .collect();
    let verification = CertificateVerificationReport {
        native_roots: captured
            .native_roots
            .unwrap_or_else(|| VerificationStatus::NotRun {
                reason: "certificate verifier was not called".to_owned(),
            }),
        mozilla_roots: captured
            .mozilla_roots
            .unwrap_or_else(|| VerificationStatus::NotRun {
                reason: "certificate verifier was not called".to_owned(),
            }),
    };

    CapturedVerification {
        certificates,
        verification,
    }
}

struct CapturedVerification {
    certificates: Vec<CertificateReport>,
    verification: CertificateVerificationReport,
}

fn certificate_report(index: usize, der: &[u8]) -> CertificateReport {
    let sha256 = sha256_hex(der);
    match X509Certificate::from_der(der) {
        Ok((_remaining, cert)) => {
            let validity = cert.validity();
            CertificateReport {
                index,
                sha256,
                subject: Some(cert.subject().to_string()),
                issuer: Some(cert.issuer().to_string()),
                serial: Some(cert.tbs_certificate.raw_serial_as_string()),
                not_before: Some(format_time(validity.not_before)),
                not_after: Some(format_time(validity.not_after)),
                subject_alt_names: subject_alt_names(&cert),
                parse_error: None,
            }
        }
        Err(error) => CertificateReport {
            index,
            sha256,
            subject: None,
            issuer: None,
            serial: None,
            not_before: None,
            not_after: None,
            subject_alt_names: SubjectAltNameReport::default(),
            parse_error: Some(error.to_string()),
        },
    }
}

fn subject_alt_names(cert: &X509Certificate<'_>) -> SubjectAltNameReport {
    let mut names = SubjectAltNameReport::default();
    let Ok(Some(extension)) = cert.subject_alternative_name() else {
        return names;
    };

    for name in &extension.value.general_names {
        match name {
            GeneralName::DNSName(name) => names.dns_names.push((*name).to_owned()),
            GeneralName::IPAddress(bytes) => names.ip_addresses.push(format_ip_san(bytes)),
            GeneralName::URI(uri) => names.uris.push((*uri).to_owned()),
            _ => {}
        }
    }

    names
}

fn format_ip_san(bytes: &[u8]) -> String {
    match bytes.len() {
        4 => IpAddr::from([bytes[0], bytes[1], bytes[2], bytes[3]]).to_string(),
        16 => {
            let mut octets = [0_u8; 16];
            octets.copy_from_slice(bytes);
            IpAddr::from(octets).to_string()
        }
        _ => hex(bytes),
    }
}

fn summarize(dns: &DnsReport, probes: &[ProbeReport]) -> SummaryReport {
    let mut diagnoses = BTreeSet::new();
    if dns.error.is_some() {
        diagnoses.insert("dns_failed".to_owned());
    }
    if dns.addresses.is_empty() && dns.error.is_none() {
        diagnoses.insert("dns_returned_no_addresses".to_owned());
    }
    if !probes.is_empty() && probes.iter().all(|probe| tcp_failed(&probe.tcp)) {
        diagnoses.insert("tcp_connect_failed_for_all_addresses".to_owned());
    }
    if probes.iter().any(tls_failed) {
        diagnoses.insert("tls_handshake_failed".to_owned());
    }
    if probes.iter().any(verification_failed) {
        diagnoses.insert("certificate_verification_failed".to_owned());
    }
    if probes.iter().any(unknown_issuer) {
        diagnoses.insert("certificate_unknown_issuer".to_owned());
    }
    if probes.iter().any(unexpected_http_status) {
        diagnoses.insert("unexpected_http_status".to_owned());
    }
    if probes.iter().any(http_bytes_failed) {
        diagnoses.insert("http_response_bytes_unavailable".to_owned());
    }
    if probes.iter().any(http_response_incomplete) {
        diagnoses.insert("http_response_incomplete".to_owned());
    }
    if diagnoses.is_empty() && probes.iter().any(probe_ok) {
        diagnoses.insert("reachable".to_owned());
    }

    let diagnoses = diagnoses.into_iter().collect::<Vec<_>>();
    SummaryReport {
        ok: diagnoses.len() == 1 && diagnoses[0] == "reachable",
        diagnoses,
    }
}

const fn tcp_failed(tcp: &TcpReport) -> bool {
    matches!(tcp, TcpReport::Failed { .. })
}

const fn tls_failed(probe: &ProbeReport) -> bool {
    matches!(probe.tls, Some(TlsReport::Failed { .. }))
}

fn verification_failed(probe: &ProbeReport) -> bool {
    verification_statuses(probe)
        .iter()
        .any(|status| matches!(status, VerificationStatus::Failed { .. }))
}

fn unknown_issuer(probe: &ProbeReport) -> bool {
    verification_statuses(probe)
        .iter()
        .any(|status| match status {
            VerificationStatus::Failed { error } => {
                let lowercase = error.to_ascii_lowercase();
                lowercase.contains("unknownissuer") || lowercase.contains("unknown issuer")
            }
            VerificationStatus::Passed | VerificationStatus::NotRun { .. } => false,
        })
}

fn unexpected_http_status(probe: &ProbeReport) -> bool {
    matches!(
        probe.http,
        Some(HttpReport::Completed {
            status_code: Some(code),
            ..
        }) if !(200..400).contains(&code)
    )
}

const fn http_bytes_failed(probe: &ProbeReport) -> bool {
    matches!(probe.http, Some(HttpReport::Failed { .. }))
}

const fn http_response_incomplete(probe: &ProbeReport) -> bool {
    matches!(
        probe.http,
        Some(HttpReport::Completed {
            response_complete: false,
            read_limit_reached: false,
            ..
        })
    )
}

fn probe_ok(probe: &ProbeReport) -> bool {
    matches!(probe.tcp, TcpReport::Connected { .. })
        && matches!(probe.tls, Some(TlsReport::Completed { .. }))
        && !verification_failed(probe)
        && matches!(
            probe.http,
            Some(
                HttpReport::Completed {
                    status_code: Some(200..=399),
                    response_complete: true,
                    ..
                } | HttpReport::Completed {
                    status_code: Some(200..=399),
                    read_limit_reached: true,
                    ..
                }
            )
        )
}

fn verification_statuses(probe: &ProbeReport) -> Vec<&VerificationStatus> {
    match &probe.tls {
        Some(
            TlsReport::Completed {
                certificate_verification,
                ..
            }
            | TlsReport::Failed {
                certificate_verification,
                ..
            },
        ) => vec![
            &certificate_verification.native_roots,
            &certificate_verification.mozilla_roots,
        ],
        None => Vec::new(),
    }
}

impl ServerCertVerifier for RecordingVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, RustlsError> {
        let peer_certificates = certificate_chain(end_entity, intermediates);
        let native_roots = verify_with_optional(
            self.native.as_deref(),
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        );
        let mozilla_roots = verify_with(
            &self.mozilla,
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        );
        if let Ok(mut capture) = self.capture.lock() {
            capture.peer_certificates = peer_certificates;
            capture.native_roots = Some(native_roots);
            capture.mozilla_roots = Some(mozilla_roots);
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        self.mozilla.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, RustlsError> {
        self.mozilla.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.mozilla.supported_verify_schemes()
    }
}

impl CertificateVerificationReport {
    fn not_run(reason: &str) -> Self {
        Self {
            native_roots: VerificationStatus::NotRun {
                reason: reason.to_owned(),
            },
            mozilla_roots: VerificationStatus::NotRun {
                reason: reason.to_owned(),
            },
        }
    }
}

impl AddressFamily {
    const fn matches(self, ip: IpAddr) -> bool {
        match self {
            Self::Any => true,
            Self::Ipv4 => ip.is_ipv4(),
            Self::Ipv6 => ip.is_ipv6(),
        }
    }
}

impl From<SocketAddr> for AddressReport {
    fn from(address: SocketAddr) -> Self {
        let ip = address.ip();
        Self {
            socket_addr: address.to_string(),
            ip: ip.to_string(),
            family: if ip.is_ipv4() { "ipv4" } else { "ipv6" },
        }
    }
}

fn verify_with_optional(
    verifier: Option<&WebPkiServerVerifier>,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    server_name: &ServerName<'_>,
    ocsp_response: &[u8],
    now: UnixTime,
) -> VerificationStatus {
    verifier.map_or_else(
        || VerificationStatus::NotRun {
            reason: "no native trust roots were available".to_owned(),
        },
        |verifier| {
            verify_with(
                verifier,
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            )
        },
    )
}

fn verify_with(
    verifier: &WebPkiServerVerifier,
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    server_name: &ServerName<'_>,
    ocsp_response: &[u8],
    now: UnixTime,
) -> VerificationStatus {
    match verifier.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now) {
        Ok(_) => VerificationStatus::Passed,
        Err(error) => VerificationStatus::Failed {
            error: error.to_string(),
        },
    }
}

fn certificate_chain(
    end_entity: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
) -> Vec<Vec<u8>> {
    let mut certificates = Vec::with_capacity(intermediates.len() + 1);
    certificates.push(end_entity.as_ref().to_vec());
    certificates.extend(
        intermediates
            .iter()
            .map(|certificate| certificate.as_ref().to_vec()),
    );
    certificates
}

fn path_and_query(url: &Url) -> String {
    let mut path = if url.path().is_empty() {
        "/".to_owned()
    } else {
        url.path().to_owned()
    };
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    path
}

fn host_header(host: &str, port: u16) -> String {
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    if port == 443 {
        host
    } else {
        format!("{host}:{port}")
    }
}

fn find_header_end(response: &[u8]) -> Option<usize> {
    response.windows(4).position(|window| window == b"\r\n\r\n")
}

fn format_time(time: ASN1Time) -> String {
    time.to_rfc2822().unwrap_or_else(|_| time.to_string())
}

fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).map_or(0, millis)
}

fn elapsed_ms(start: Instant) -> u64 {
    millis(start.elapsed())
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex(Sha256::digest(bytes).as_slice())
}

fn hex_prefix(bytes: &[u8]) -> String {
    hex(&bytes[..bytes.len().min(RESPONSE_PREFIX_BYTES)])
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_status_headers_and_body() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nhello";
        let HttpReport::Completed {
            status_code,
            headers,
            body_sample_bytes,
            body_sample_complete,
            ..
        } = parse_http_response(response, 32, true, false, 7)
        else {
            panic!("expected completed HTTP report");
        };

        assert_eq!(status_code, Some(200));
        assert_eq!(headers[0].name, "Content-Type");
        assert_eq!(headers[0].value, "text/plain");
        assert_eq!(body_sample_bytes, 5);
        assert!(body_sample_complete);
    }

    #[test]
    fn marks_body_sample_incomplete_when_read_stopped_at_limit() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 1024\r\n\r\nabc";
        let HttpReport::Completed {
            body_sample_bytes,
            body_sample_complete,
            ..
        } = parse_http_response(response, 3, false, true, 7)
        else {
            panic!("expected completed HTTP report");
        };

        assert_eq!(body_sample_bytes, 3);
        assert!(!body_sample_complete);
    }

    #[test]
    fn skips_interim_http_headers() {
        let response = b"HTTP/1.1 103 Early Hints\r\nLink: </style.css>\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nok";
        let HttpReport::Completed {
            status_code,
            headers,
            body_sample_bytes,
            ..
        } = parse_http_response(response, 32, true, false, 7)
        else {
            panic!("expected completed HTTP report");
        };

        assert_eq!(status_code, Some(200));
        assert_eq!(headers[0].name, "Content-Type");
        assert_eq!(headers[0].value, "text/plain");
        assert_eq!(body_sample_bytes, 2);
    }

    #[test]
    fn detects_complete_content_length_response() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert!(response_has_complete_framing(response));

        let incomplete = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhe";
        assert!(!response_has_complete_framing(incomplete));
        assert!(!eof_completes_response(incomplete));
    }

    #[test]
    fn detects_complete_chunked_response() {
        let response =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert!(response_has_complete_framing(response));

        let incomplete = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n";
        assert!(!response_has_complete_framing(incomplete));
        assert!(!eof_completes_response(incomplete));
    }

    #[test]
    fn treats_no_body_status_as_complete() {
        let response = b"HTTP/1.1 205 Reset Content\r\n\r\n";
        assert!(response_has_complete_framing(response));
    }

    #[test]
    fn transfer_encoding_overrides_content_length() {
        let incomplete = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n";
        assert!(!response_has_complete_framing(incomplete));

        let complete = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
        assert!(response_has_complete_framing(complete));
    }

    #[test]
    fn rejects_chunk_size_that_would_overflow() {
        let response = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nffffffffffffffff\r\n";
        assert!(!response_has_complete_framing(response));
    }

    #[test]
    fn keeps_query_in_request_target() {
        let url = Url::parse("https://ix.dev/cli/linux-x86_64/ix?download=1").unwrap();
        assert_eq!(path_and_query(&url), "/cli/linux-x86_64/ix?download=1");
    }
}
