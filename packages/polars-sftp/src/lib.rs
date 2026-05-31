//! The Rust core behind `scan_sftp`: read a remote file over SFTP and hand it to
//! Polars as a `DataFrame`.
//!
//! This is deliberately thin. It opens an SFTP connection, pulls the file, and
//! decodes it with Polars' own readers (parquet / IPC / CSV), applying column
//! projection and a row limit. The lazy plumbing, schema probing, and predicate
//! filtering live in the Python wrapper (`polars_sftp/__init__.py`) via
//! `register_io_source`, because Polars' IO-plugin interface is Python by design.
//!
//! v1 fetches the whole remote file into memory, then decodes from a `Cursor`:
//! Polars' parquet/IPC readers require `MmapBytesReader`, which an `ssh2::File`
//! (a plain `Read + Seek`) is not. That means projection trims decode and output,
//! not bytes transferred. Selective range-reads over SFTP (a custom
//! `MmapBytesReader` over `ssh2` seek) are a future optimization.

use std::io::{Cursor, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use polars::prelude::*;
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3_polars::PyDataFrame;

/// Supported remote file formats, inferred from the path extension or an explicit
/// `storage_format` hint.
#[derive(Clone, Copy)]
enum Format {
    Parquet,
    Ipc,
    Csv,
    /// Newline-delimited JSON (one JSON object per line), e.g. Claude history.
    Ndjson,
}

fn resolve_format(hint: Option<&str>, path: &str) -> PyResult<Format> {
    let ext = hint
        .map(str::to_ascii_lowercase)
        .or_else(|| path.rsplit('.').next().map(str::to_ascii_lowercase));
    match ext.as_deref() {
        Some("parquet" | "pq") => Ok(Format::Parquet),
        Some("ipc" | "arrow" | "feather") => Ok(Format::Ipc),
        Some("csv" | "tsv" | "txt") => Ok(Format::Csv),
        Some("jsonl" | "ndjson") => Ok(Format::Ndjson),
        other => Err(PyValueError::new_err(format!(
            "polars-sftp: cannot infer a format from {other:?}; pass storage_format=\"parquet\"|\"ipc\"|\"csv\"|\"ndjson\""
        ))),
    }
}

/// Open an authenticated SFTP session and read `remote_path` fully into memory.
/// Auth order: explicit password, then private-key file, then the SSH agent.
#[allow(clippy::too_many_arguments)]
fn fetch_bytes(
    host: &str,
    port: u16,
    username: &str,
    password: Option<&str>,
    private_key: Option<&str>,
    remote_path: &str,
    timeout_ms: u64,
    check_host_key: bool,
) -> Result<Vec<u8>, String> {
    // Bound every blocking step: the TCP connect, the raw socket reads/writes,
    // and libssh2's own blocking calls. Without this a stuck or silent peer hangs
    // the Polars query forever.
    let timeout = Duration::from_millis(timeout_ms);
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| format!("resolve {host}:{port}: {e}"))?
        .next()
        .ok_or_else(|| format!("no address resolved for {host}:{port}"))?;
    let tcp = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("connect {host}:{port}: {e}"))?;
    tcp.set_read_timeout(Some(timeout)).ok();
    tcp.set_write_timeout(Some(timeout)).ok();

    let mut sess = ssh2::Session::new().map_err(|e| format!("ssh session: {e}"))?;
    // libssh2 blocking-call timeout, in milliseconds (0 would mean no timeout).
    sess.set_timeout(timeout_ms.clamp(1, u32::MAX as u64) as u32);
    sess.set_tcp_stream(tcp);
    sess.handshake().map_err(|e| format!("ssh handshake: {e}"))?;

    // Verify the server's host key against ~/.ssh/known_hosts BEFORE sending any
    // credential, so a MITM of a known host is rejected rather than handed a
    // password. An unknown host is accepted (trust on first use); a key that
    // mismatches a recorded entry is refused.
    if check_host_key {
        verify_host_key(&sess, host, port)?;
    }

    if let Some(pw) = password {
        sess.userauth_password(username, pw)
            .map_err(|e| format!("password auth for {username}: {e}"))?;
    } else if let Some(key) = private_key {
        sess.userauth_pubkey_file(username, None, Path::new(key), None)
            .map_err(|e| format!("key auth for {username} with {key}: {e}"))?;
    } else {
        sess.userauth_agent(username)
            .map_err(|e| format!("agent auth for {username}: {e}"))?;
    }
    if !sess.authenticated() {
        return Err(format!("ssh authentication failed for {username}@{host}"));
    }

    let sftp = sess.sftp().map_err(|e| format!("open sftp: {e}"))?;
    let mut file = sftp
        .open(Path::new(remote_path))
        .map_err(|e| format!("open {remote_path}: {e}"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| format!("read {remote_path}: {e}"))?;
    Ok(bytes)
}

/// Verify the connected server's host key against `~/.ssh/known_hosts`. A recorded
/// entry that mismatches is rejected (possible MITM); an unrecorded host is
/// accepted (trust on first use). A missing known_hosts file means no recorded
/// entries, so every host is first-use.
fn verify_host_key(sess: &ssh2::Session, host: &str, port: u16) -> Result<(), String> {
    let mut known = sess
        .known_hosts()
        .map_err(|e| format!("init known_hosts: {e}"))?;
    if let Some(home) = std::env::var_os("HOME") {
        let path = PathBuf::from(home).join(".ssh/known_hosts");
        if path.exists() {
            known
                .read_file(&path, ssh2::KnownHostFileKind::OpenSSH)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
        }
    }
    let (key, _) = sess
        .host_key()
        .ok_or_else(|| "server presented no host key".to_string())?;
    match known.check_port(host, port, key) {
        ssh2::CheckResult::Match | ssh2::CheckResult::NotFound => Ok(()),
        ssh2::CheckResult::Mismatch => Err(format!(
            "host key mismatch for {host}:{port} (possible MITM); fix ~/.ssh/known_hosts or pass check_host_key=False"
        )),
        ssh2::CheckResult::Failure => Err(format!("host key check failed for {host}:{port}")),
    }
}

/// Decode `bytes` as `format`, applying column projection and a row limit. For
/// parquet the projection/limit are pushed into the reader; for every format they
/// are re-applied to the resulting frame so the contract holds uniformly.
fn decode(
    bytes: Vec<u8>,
    format: Format,
    with_columns: Option<Vec<String>>,
    n_rows: Option<usize>,
) -> PolarsResult<DataFrame> {
    let cursor = Cursor::new(bytes);
    let mut df = match format {
        Format::Parquet => {
            // Projection is pushed into the reader (skips decoding other columns);
            // the row limit is applied below via `head`, since polars 0.53's
            // `ParquetReader` has no row-limit setter.
            let mut reader = ParquetReader::new(cursor);
            if let Some(cols) = with_columns.clone() {
                reader = reader.with_columns(Some(cols));
            }
            reader.finish()?
        }
        Format::Ipc => IpcReader::new(cursor).finish()?,
        Format::Csv => CsvReadOptions::default()
            .into_reader_with_file_handle(cursor)
            .finish()?,
        // Infer the schema from the whole file (`None`), not a prefix: heterogeneous
        // history files often introduce a key only on a late line, and a bounded
        // window would silently drop it. Lines missing a key read back null; a key
        // with conflicting types across the file surfaces as a decode error (which
        // the caller can catch per file) rather than corrupting the frame.
        Format::Ndjson => JsonReader::new(cursor)
            .with_json_format(JsonFormat::JsonLines)
            .infer_schema_len(None)
            .finish()?,
    };

    if let Some(cols) = with_columns {
        df = df.select(cols)?;
    }
    if let Some(n) = n_rows {
        df = df.head(Some(n));
    }
    Ok(df)
}

/// Read a remote file over SFTP into a Polars `DataFrame`.
///
/// `with_columns` projects (the reader/output keeps only these), `n_rows` caps the
/// row count (pass `0` to read just the schema). Auth: `password`, else
/// `private_key` file, else the SSH agent. `username` defaults to `$USER`.
#[pyfunction]
#[pyo3(signature = (
    host,
    path,
    *,
    port = 22,
    username = None,
    password = None,
    private_key = None,
    storage_format = None,
    with_columns = None,
    n_rows = None,
    timeout_ms = 30_000,
    check_host_key = true,
))]
#[allow(clippy::too_many_arguments)]
fn read_sftp(
    py: Python<'_>,
    host: String,
    path: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
    private_key: Option<String>,
    storage_format: Option<String>,
    with_columns: Option<Vec<String>>,
    n_rows: Option<usize>,
    timeout_ms: u64,
    check_host_key: bool,
) -> PyResult<PyDataFrame> {
    let user = username
        .or_else(|| std::env::var("USER").ok())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| PyValueError::new_err("polars-sftp: no username given and $USER is unset"))?;
    let format = resolve_format(storage_format.as_deref(), &path)?;

    // Release the GIL for the blocking network read + decode: other Python
    // threads (and Polars' own engine) keep running while we wait on the socket.
    let df = py
        .detach(|| {
            let bytes = fetch_bytes(
                &host,
                port,
                &user,
                password.as_deref(),
                private_key.as_deref(),
                &path,
                timeout_ms,
                check_host_key,
            )?;
            decode(bytes, format, with_columns, n_rows)
                .map_err(|e| format!("decode {path}: {e}"))
        })
        .map_err(|e| PyRuntimeError::new_err(format!("polars-sftp: {e}")))?;
    Ok(PyDataFrame(df))
}

#[pymodule]
fn _polars_sftp(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(read_sftp, module)?)?;
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
