//! Shared ix-mcp kernel dispatch plumbing for the out-of-band Stop hooks
//! (`retro-gate` in `retro.rs`, `friction-report` in `friction.rs`).
//!
//! Both hooks follow the same pattern: a detached worker ships a payload to
//! the fleet's public ix-mcp streamable-HTTP kernel and opens a
//! `fabric.claude.session` agent there to do the slow, credentialed work
//! server-side (a journal-recorded, interruptible Claude Agent SDK session in
//! the kernel process). This module owns the pieces they share:
//!
//! - a minimal streamable-HTTP MCP client ([`McpClient`]) on the crate's
//!   existing blocking reqwest: initialize (capture `Mcp-Session-Id`),
//!   `notifications/initialized`, then `tools/call`, with responses arriving
//!   as plain JSON or SSE bodies ([`parse_rpc_response`]);
//! - the payload pipeline: gzip -> base64 -> chunked `python_exec` cells
//!   accumulating in a kernel variable -> weave CAS (`weave.put_blob`) ->
//!   `fabric.claude.session()` with the CAS hash substituted into the session
//!   prompt server-side (the call creates the work; the journal records it --
//!   no dispatcher; a spawned kernel job awaits the session's result and
//!   closes it, so the run settles to a terminal fact without anyone
//!   attached);
//! - the env-backed config both hooks read: `IX_MCP_URL` and
//!   `IX_MCP_API_KEY`/`IX_MCP_API_KEY_FILE`.
//!
//! Transcript shipping rationale (why CAS and not a kernel-cwd file): the
//! kernel behind `IX_MCP_URL` runs on a fleet host that cannot read the origin
//! machine's filesystem, and the spawned agent's working context is its own,
//! not this filesystem. The one data plane both ends verifiably share is the
//! weave journal itself: chunks ride ONE MCP session (one kernel namespace),
//! the kernel stores the reassembled blob in CAS, and the session prompt tells
//! the spawned agent to `weave.get_blob` it back out in its own kernel.
//!
//! Failure policy matches the rest of the crate: every error logs (through the
//! caller-supplied log sink) and returns `None`; nothing here ever touches
//! stdout/stderr or blocks Stop. A machine without fleet creds (`api_key()`
//! returns `None`) simply never dispatches.

use std::fs;
use std::path::Path;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};

/// The fleet's public ix-mcp streamable-HTTP endpoint (mcp.ix.dev vhost ->
/// ix-mcp-public unit). Overridable via `IX_MCP_URL` for tests and self-hosted
/// kernels.
const DEFAULT_MCP_URL: &str = "https://mcp.ix.dev/mcp";

/// Base64 characters per `python_exec` upload call (~1.5 MiB of gzipped
/// payload each). The fronting nginx has no body cap (`client_max_body_size
/// 0`), so this only keeps a single JSON-RPC message comfortably sized for the
/// transport and kernel; tens-of-MB transcripts arrive as a handful of chunks.
const CHUNK_B64_CHARS: usize = 2_000_000;

/// Whole-request timeout. The final `python_exec` (CAS upload + session open)
/// runs inside the POST with a kernel budget of [`FINAL_BUDGET_SECS`] plus the
/// server's wedge grace, so the HTTP timeout must sit above both.
const HTTP_TIMEOUT: Duration = Duration::from_mins(3);

/// Kernel budget for a chunk-append call (string append: fast).
const CHUNK_BUDGET_SECS: f64 = 30.0;
/// Kernel budget for the finalize call (CAS put + session open); the server
/// clamps to its `max_budget` (120s) anyway.
const FINAL_BUDGET_SECS: f64 = 120.0;

// --- env-backed config ---

pub fn mcp_url() -> String {
    std::env::var("IX_MCP_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_MCP_URL.to_owned())
}

/// The ix-mcp API key: `IX_MCP_API_KEY`, else the contents of
/// `IX_MCP_API_KEY_FILE` (the same pair the server itself reads). `None` means
/// this machine has no fleet creds and the dispatch quietly does nothing.
pub fn api_key() -> Option<String> {
    if let Some(key) = std::env::var("IX_MCP_API_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        return Some(key.trim().to_owned());
    }
    let path = std::env::var_os("IX_MCP_API_KEY_FILE").filter(|v| !v.is_empty())?;
    read_key_file(Path::new(&path))
}

fn read_key_file(path: &Path) -> Option<String> {
    let key = fs::read_to_string(path).ok()?;
    let key = key.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_owned())
    }
}

// --- small shared helpers ---

pub fn truncate_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// First 8 chars of the session id, the human-readable label suffix.
pub fn session_prefix(session: &str) -> String {
    session.chars().take(8).collect()
}

/// A kernel variable name derived from the hook name and session id:
/// alphanumerics kept, everything else `_`, so a UUID-shaped id is a valid
/// Python identifier tail (e.g. `__retro_b64_0af5c2de`).
pub fn python_var(hook: &str, session: &str) -> String {
    let tail: String = session
        .chars()
        .take(8)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("__{hook}_b64_{tail}")
}

pub fn gzip_bytes(data: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write as _;

    use flate2::{Compression, write::GzEncoder};
    let mut enc = GzEncoder::new(Vec::new(), Compression::new(6));
    enc.write_all(data).ok()?;
    enc.finish().ok()
}

/// Split a base64 string (pure ASCII, so byte slicing is char-safe) into
/// `size`-char pieces; always at least one piece so an empty payload still
/// produces a well-formed upload.
fn chunk_b64(b64: &str, size: usize) -> Vec<&str> {
    if b64.is_empty() {
        return vec![""];
    }
    b64.as_bytes()
        .chunks(size)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect()
}

/// One upload cell: the first defines the accumulator, every later one appends.
/// The chunk is base64 (alphanumeric + `+/=`), safe inside double quotes.
fn chunk_code(var: &str, chunk: &str, first: bool) -> String {
    if first {
        format!("{var} = [\"{chunk}\"]\nprint(len({var}))")
    } else {
        format!("{var}.append(\"{chunk}\")\nprint(len({var}))")
    }
}

/// The finalize cell: decode the accumulated base64, store the gzipped payload
/// in weave CAS, and open the follow-up agent as a `fabric.claude.session`
/// with the CAS hash substituted into the prompt (the call creates the work;
/// the journal records it -- no dispatcher). The session lives in the kernel
/// process: a spawned kernel job awaits its result and closes it, so the run
/// settles to a terminal fact without anyone attached. The prompt itself
/// travels base64-encoded so no escaping of its prose is ever needed.
fn finalize_code(spec: &DispatchSpec, prompt: &str) -> String {
    let prompt_b64 = B64.encode(prompt);
    let DispatchSpec {
        var,
        placeholder,
        job_name,
        tag,
        ..
    } = spec;
    format!(
        r#"import base64 as _b64
import weave
import fabric
_gz = _b64.b64decode("".join({var}))
del {var}
_hash = await weave.put_blob(_gz)
_prompt = _b64.b64decode("{prompt_b64}").decode("utf-8").replace("{placeholder}", _hash)
_s = await fabric.claude.session(_prompt)

async def _finish() -> str:
    try:
        return await _s.result()
    finally:
        await _s.close()

jobs.spawn(_finish(), name="{job_name}")
print("{tag} dispatched: blob=" + _hash + " task=" + _s.task + " gz_bytes=" + str(len(_gz)))"#
    )
}

// --- high-level dispatch driver ---

/// Everything hook-specific about one kernel dispatch; the driver
/// ([`ship_and_delegate`]) is the shared flow.
pub struct DispatchSpec<'a> {
    /// `clientInfo.name` in the MCP initialize request.
    pub client_name: &'a str,
    /// `session_set_name` label (the server gates acting tools on it).
    pub label: &'a str,
    /// `topic_set` topic (the server gates acting tools on it too).
    pub topic: &'a str,
    /// Kernel accumulator variable, from [`python_var`].
    pub var: &'a str,
    /// Short hook tag used in `python_exec` intents and the finalize print.
    pub tag: &'a str,
    /// Placeholder token in the session prompt that the finalize cell replaces
    /// with the CAS hash server-side (the hash only exists after `put_blob`).
    pub placeholder: &'a str,
    /// `jobs.spawn(..., name=)` for the kernel job that awaits and closes the
    /// spawned `fabric.claude.session`.
    pub job_name: &'a str,
    /// Intent string for the finalize `python_exec` call.
    pub final_intent: &'a str,
    /// Log-line prefix (the origin session id).
    pub ctx: &'a str,
    /// The calling hook's log sink (its `<state>/<hook>.log` appender).
    pub log: fn(&str),
}

/// The finalize call's tool result plus the shipping stats the callers log.
pub struct DispatchOutcome {
    pub result: Value,
    pub chunks: usize,
    pub gz_bytes: usize,
}

/// The shared dispatch flow: gzip `data`, connect an MCP session, set its name
/// and topic, upload the base64 chunks into the kernel accumulator, then run
/// the finalize cell (CAS `put_blob` + `fabric.claude.session(prompt)`). Every
/// failure logs through `spec.log` and returns `None`.
pub fn ship_and_delegate(
    spec: &DispatchSpec,
    url: &str,
    key: &str,
    data: &[u8],
    prompt: &str,
) -> Option<DispatchOutcome> {
    let log = spec.log;
    let ctx = spec.ctx;

    let Some(gz) = gzip_bytes(data) else {
        log(&format!("{ctx}: gzip failed"));
        return None;
    };
    let b64 = B64.encode(&gz);
    let chunks = chunk_b64(&b64, CHUNK_B64_CHARS);
    let total = chunks.len();

    let Some(mut client) = McpClient::connect(url, key, spec.client_name, log) else {
        log(&format!("{ctx}: could not initialize MCP session at {url}"));
        return None;
    };

    // The server gates acting tools on a session name AND a topic; set both
    // before the first python_exec.
    if client
        .call_tool("session_set_name", &json!({ "name": spec.label }))
        .is_none()
    {
        log(&format!("{ctx}: session_set_name failed"));
        return None;
    }
    if client
        .call_tool("topic_set", &json!({ "topic": spec.topic }))
        .is_none()
    {
        log(&format!("{ctx}: topic_set failed"));
        return None;
    }

    // All chunk cells ride ONE MCP session on purpose: the HTTP transport
    // gives each MCP session its own kernel namespace, so the accumulator
    // variable is only visible to calls carrying the same Mcp-Session-Id.
    for (i, chunk) in chunks.iter().enumerate() {
        let code = chunk_code(spec.var, chunk, i == 0);
        let intent = format!("{}: upload transcript chunk {}/{total}", spec.tag, i + 1);
        if client
            .call_tool(
                "python_exec",
                &json!({ "code": code, "budget": CHUNK_BUDGET_SECS, "intent": intent }),
            )
            .is_none()
        {
            log(&format!("{ctx}: chunk {}/{total} upload failed", i + 1));
            return None;
        }
    }

    let code = finalize_code(spec, prompt);
    let Some(result) = client.call_tool(
        "python_exec",
        &json!({
            "code": code,
            "budget": FINAL_BUDGET_SECS,
            "intent": spec.final_intent,
        }),
    ) else {
        log(&format!("{ctx}: finalize (put_blob + session open) failed"));
        return None;
    };
    Some(DispatchOutcome {
        result,
        chunks: total,
        gz_bytes: gz.len(),
    })
}

// --- minimal streamable-HTTP MCP client ---

/// Just enough MCP-over-streamable-HTTP for these dispatches: initialize
/// (capture `Mcp-Session-Id`), `notifications/initialized`, then `tools/call`.
/// Built on the crate's existing blocking reqwest; responses may arrive as
/// plain JSON or as an SSE body, both handled by [`parse_rpc_response`].
pub struct McpClient {
    http: reqwest::blocking::Client,
    url: String,
    key: String,
    session_id: Option<String>,
    next_id: u64,
    log: fn(&str),
}

/// One raw HTTP exchange with the MCP endpoint: response status and body text.
/// A named struct because the house clippy fork forbids anonymous multi-value
/// tuple returns.
struct HttpReply {
    status: u16,
    body: String,
}

impl McpClient {
    pub fn connect(url: &str, key: &str, client_name: &str, log: fn(&str)) -> Option<Self> {
        let http = reqwest::blocking::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .ok()?;
        let mut client = Self {
            http,
            url: url.to_owned(),
            key: key.to_owned(),
            session_id: None,
            next_id: 1,
            log,
        };
        client.request(
            "initialize",
            &json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": client_name,
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )?;
        // The SDK requires the initialized notification before any request.
        client.notify("notifications/initialized");
        Some(client)
    }

    fn post(&mut self, body: &Value) -> Option<HttpReply> {
        let mut req = self
            .http
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("X-Api-Key", &self.key);
        if let Some(sid) = &self.session_id {
            req = req.header("Mcp-Session-Id", sid.clone());
        }
        let resp = match req.json(body).send() {
            Ok(r) => r,
            Err(e) => {
                (self.log)(&format!("POST {} failed: {e}", self.url));
                return None;
            }
        };
        // The initialize response carries the session id every later request
        // must echo; the header name is case-insensitive in reqwest.
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.session_id = Some(sid.to_owned());
        }
        let status = resp.status().as_u16();
        let body = resp.text().unwrap_or_default();
        Some(HttpReply { status, body })
    }

    /// One JSON-RPC request; returns the `result` value or logs and None.
    fn request(&mut self, method: &str, params: &Value) -> Option<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let HttpReply { status, body: text } = self.post(&body)?;
        if !(200..300).contains(&status) {
            (self.log)(&format!(
                "{method} -> HTTP {status}: {}",
                truncate_chars(&text, 300)
            ));
            return None;
        }
        let Some(msg) = parse_rpc_response(&text, id) else {
            (self.log)(&format!(
                "{method}: no JSON-RPC reply for id {id} in body: {}",
                truncate_chars(&text, 300)
            ));
            return None;
        };
        if let Some(err) = msg.get("error") {
            (self.log)(&format!(
                "{method} -> JSON-RPC error: {}",
                truncate_chars(&err.to_string(), 300)
            ));
            return None;
        }
        msg.get("result").cloned()
    }

    /// Fire-and-forget notification (no id, 202-shaped reply).
    fn notify(&mut self, method: &str) {
        let body = json!({ "jsonrpc": "2.0", "method": method });
        let _ = self.post(&body);
    }

    /// `tools/call`, treating a tool-level `isError` result as failure.
    fn call_tool(&mut self, name: &str, arguments: &Value) -> Option<Value> {
        let result = self.request(
            "tools/call",
            &json!({ "name": name, "arguments": arguments }),
        )?;
        if result
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let content = serde_json::to_string(result.get("content").unwrap_or(&Value::Null))
                .unwrap_or_default();
            (self.log)(&format!(
                "tool {name} errored: {}",
                truncate_chars(&content, 300)
            ));
            return None;
        }
        Some(result)
    }
}

/// Extract the JSON-RPC message answering `id` from a streamable-HTTP body:
/// either a plain JSON object, or an SSE stream whose `data:` lines carry JSON
/// messages (server pings and unrelated messages are skipped).
fn parse_rpc_response(body: &str, id: u64) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(body.trim())
        && rpc_id_matches(&v, id)
    {
        return Some(v);
    }
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        if let Ok(v) = serde_json::from_str::<Value>(data.trim())
            && rpc_id_matches(&v, id)
        {
            return Some(v);
        }
    }
    None
}

fn rpc_id_matches(v: &Value, id: u64) -> bool {
    v.get("id").and_then(Value::as_u64) == Some(id)
}

#[cfg(test)]
mod tests {
    use super::{
        DispatchSpec, chunk_b64, chunk_code, finalize_code, gzip_bytes, parse_rpc_response,
        python_var, read_key_file, session_prefix,
    };
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as B64;
    use serde_json::json;

    fn noop_log(_: &str) {}

    fn spec<'a>(var: &'a str, placeholder: &'a str) -> DispatchSpec<'a> {
        DispatchSpec {
            client_name: "claude-hooks-test",
            label: "test-label",
            topic: "test topic",
            var,
            tag: "session-retro",
            placeholder,
            job_name: "session-retro",
            final_intent: "test finalize",
            ctx: "s1",
            log: noop_log,
        }
    }

    #[test]
    fn chunking_covers_the_whole_string() {
        // remainder chunk
        let s = "a".repeat(10);
        let chunks = chunk_b64(&s, 4);
        assert_eq!(chunks, vec!["aaaa", "aaaa", "aa"]);
        assert_eq!(chunks.concat(), s);
        // exact multiple: no trailing empty chunk
        let s = "b".repeat(8);
        assert_eq!(chunk_b64(&s, 4).len(), 2);
        // smaller than one chunk
        assert_eq!(chunk_b64("xy", 4), vec!["xy"]);
        // empty input still yields one well-formed (empty) chunk
        assert_eq!(chunk_b64("", 4), vec![""]);
    }

    #[test]
    fn chunk_code_defines_then_appends() {
        let first = chunk_code("__retro_b64_s1", "AAAA", true);
        assert!(first.contains("__retro_b64_s1 = [\"AAAA\"]"), "{first}");
        let later = chunk_code("__retro_b64_s1", "BBBB", false);
        assert!(later.contains("__retro_b64_s1.append(\"BBBB\")"), "{later}");
        assert!(!later.contains("= ["), "{later}");
    }

    #[test]
    fn finalize_code_ships_prompt_base64_and_opens_session() {
        let prompt = "walk the transcript at __X_BLOB__ \"quoted\" text";
        let code = finalize_code(&spec("__retro_b64_s1", "__X_BLOB__"), prompt);
        // decodes to CAS put + a fabric claude session on the accumulated chunks
        assert!(code.contains("\"\".join(__retro_b64_s1)"), "{code}");
        assert!(code.contains("weave.put_blob"), "{code}");
        assert!(code.contains("fabric.claude.session"), "{code}");
        // the session settles unattended: a kernel job awaits and closes it
        assert!(code.contains("jobs.spawn"), "{code}");
        assert!(code.contains("name=\"session-retro\""), "{code}");
        // the prompt rides base64 so its quotes never need escaping
        assert!(!code.contains("\"quoted\""), "{code}");
        let b64 = B64.encode(prompt);
        assert!(code.contains(&b64), "{code}");
        // the kernel substitutes the hash placeholder after put_blob
        assert!(code.contains("__X_BLOB__"), "{code}");
    }

    #[test]
    fn python_var_sanitizes_uuid_session_ids() {
        assert_eq!(
            python_var("retro", "0af5c2de-1234-5678"),
            "__retro_b64_0af5c2de"
        );
        assert_eq!(python_var("friction", "ab-cd"), "__friction_b64_ab_cd");
        assert_eq!(session_prefix("0af5c2de-1234"), "0af5c2de");
        assert_eq!(session_prefix("s1"), "s1");
    }

    #[test]
    fn gzip_roundtrips_through_flate2() {
        let data = b"line one\nline two\n".repeat(100);
        let gz = gzip_bytes(&data).expect("gzip");
        assert!(gz.len() < data.len());
        let mut dec = flate2::read::GzDecoder::new(&gz[..]);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut dec, &mut out).expect("gunzip");
        assert_eq!(out, data);
    }

    #[test]
    fn parses_plain_json_and_sse_rpc_replies() {
        // plain JSON body
        let body = r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#;
        let msg = parse_rpc_response(body, 3).expect("plain json");
        assert_eq!(msg["result"]["ok"], json!(true));
        // SSE body with a ping, an unrelated message, then the answer
        let sse = concat!(
            ": ping\n\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":{\"content\":[]}}\n\n",
        );
        let msg = parse_rpc_response(sse, 7).expect("sse");
        assert!(msg["result"]["content"].is_array());
        // wrong id -> None
        assert!(parse_rpc_response(sse, 8).is_none());
        assert!(parse_rpc_response("not json at all", 1).is_none());
    }

    #[test]
    fn key_file_reads_trimmed_and_rejects_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("key");
        std::fs::write(&path, "  sekrit-123\n").expect("write");
        assert_eq!(read_key_file(&path).as_deref(), Some("sekrit-123"));
        std::fs::write(&path, "   \n").expect("write");
        assert_eq!(read_key_file(&path), None);
        assert_eq!(read_key_file(dir.path().join("missing").as_path()), None);
    }
}
