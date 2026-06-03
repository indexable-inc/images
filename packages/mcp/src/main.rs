use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, ExitCode, Stdio},
    sync::{
        Arc, Mutex, TryLockError,
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::{
        stdio,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
        },
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

mod exec_board;

const DEFAULT_SESSION_ID: &str = "default";
const WORKER_SOURCE: &str = include_str!("python_worker.py");

/// Default wall-clock budget for a single `python_eval`/`python_exec` call. A
/// call that runs longer is abandoned: the caller gets a timeout error and the
/// worker is restarted, so the next call is not stuck behind the hung one. Raise
/// it per call with `timeout_secs` for legitimately long work (training, large
/// downloads). There is deliberately no "infinite" setting, so a runaway cell
/// cannot wedge the session forever.
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Default budget for the `search_*` tools. The first cold index of a large
/// checkout can take minutes (a full Mixedbread reconcile), so these get more
/// headroom than a plain `python_exec`. Still overridable per call.
const SEARCH_TIMEOUT_SECS: u64 = 600;

/// Budget for internal control round-trips: the startup `ping`, `reset`, and the
/// graceful `close`. These do no user work, so they answer promptly; a worker
/// that cannot is treated as broken and replaced.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on a caller-supplied `timeout_secs`. A budget beyond this is
/// clamped rather than honoured: 24h is longer than any single MCP call has any
/// business running (a longer job should be polled, not held open), and clamping
/// keeps `Instant::now() + timeout` from overflowing, which panics. Without the
/// clamp a single call with `timeout_secs` near `u64::MAX` panics inside the held
/// session lock and poisons it, bricking the session.
const MAX_TIMEOUT: Duration = Duration::from_hours(24);

/// How often the blocking wait wakes to re-check the deadline and the
/// cancellation token. A client cancel (`notifications/cancelled`) or an elapsed
/// deadline is therefore noticed within roughly this interval rather than only
/// when the worker happens to reply.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Address bound when `serve --http` is passed without an explicit value. Loopback
/// only, matching the Streamable HTTP server's default loopback `Host` allowlist
/// (DNS-rebinding guard); a public bind needs both a routable address here and a
/// matching `allowed_hosts` override.
const DEFAULT_HTTP_ADDRESS: &str = "127.0.0.1:8000";

/// Path the Streamable HTTP endpoint is mounted at, the de facto MCP convention.
const HTTP_MCP_PATH: &str = "/mcp";

// Surfaced to clients on initialize so an agent discovers the preinstalled
// packages without reading the build. `tui` is the bundled PTY driver; naming
// it here keeps one home for the fact instead of repeating it across every
// `python_*` tool description.
const SERVER_INSTRUCTIONS: &str = include_str!("server_instructions.txt");

/// Sink for interim stdout streamed from a running cell: each chunk is handed to
/// this callback as it arrives, before the final response. `call_content` points
/// it at the dashboard pane; other callers stream nowhere and pass `None`.
type PartialSink = Box<dyn FnMut(&str) + Send>;

/// A token that is never cancelled, for internal round-trips (startup ping,
/// reset, close, and every CLI call) that are not tied to a client request.
fn uncancellable() -> CancellationToken {
    CancellationToken::new()
}

/// Translate a caller-supplied `timeout_secs` into a duration, clamping unset or
/// `0` to `default_secs` and capping at [`MAX_TIMEOUT`]. `0` means "use the
/// default" rather than "no timeout": an instant timeout and an infinite one are
/// both footguns the tool should not hand out. The cap also keeps the later
/// `Instant + Duration` from overflowing on an absurd budget.
fn call_timeout(timeout_secs: Option<u64>, default_secs: u64) -> Duration {
    let secs = timeout_secs.filter(|secs| *secs > 0).unwrap_or(default_secs);
    Duration::from_secs(secs).min(MAX_TIMEOUT)
}

#[derive(Parser)]
#[command(name = "ix-mcp")]
struct Cli {
    #[arg(
        long,
        global = true,
        help = "Working directory for CLI eval/exec/repl sessions."
    )]
    cwd: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Subcommand)]
enum CliCommand {
    Serve {
        /// Serve over Streamable HTTP instead of stdio. Pass the flag bare to
        /// bind the loopback default, or give an explicit `host:port`. Stdio
        /// stays the default when the flag is absent, so existing MCP clients
        /// that launch `ix-mcp` over a pipe are unaffected.
        #[arg(
            long,
            value_name = "ADDR",
            num_args = 0..=1,
            default_missing_value = DEFAULT_HTTP_ADDRESS,
        )]
        http: Option<String>,
    },
    Repl,
    Eval { expression: String },
    Exec { source: String },
}

#[derive(Clone)]
struct McpServer {
    sessions: Arc<Mutex<SessionManager>>,
    tool_router: ToolRouter<Self>,
    // The process-global dashboard producer, when one bound. Every HTTP session's
    // server shares it, so one socket carries every session's exec panes.
    board: Option<&'static exec_board::ExecBoard>,
}

impl McpServer {
    fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(SessionManager::default())),
            tool_router: Self::tool_router(),
            board: exec_board::global(),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(SERVER_INSTRUCTIONS)
    }
}

#[tool_router]
impl McpServer {
    #[tool(
        description = "Create a persistent Python session on the pinned interpreter. The interpreter is fixed; each session runs in its own writable venv so `pip install` works."
    )]
    async fn python_session_create(
        &self,
        Parameters(request): Parameters<CreateSessionRequest>,
    ) -> String {
        self.with_registry(move |sessions| {
            sessions.create_arc(request.session_id, request.cwd)?;
            Ok("session ready".to_string())
        })
        .await
    }

    #[tool(description = "List persistent Python sessions.")]
    async fn python_session_list(&self) -> String {
        self.with_registry(SessionManager::list).await
    }

    #[tool(description = "Close a persistent Python session.")]
    async fn python_session_close(&self, Parameters(request): Parameters<SessionRequest>) -> String {
        self.with_registry(move |sessions| sessions.close(&request.session_id))
            .await
    }

    #[tool(
        description = "Evaluate a Python expression in a persistent session. Pass a one-line `intent` describing what the call is for; it titles the run's dashboard card. Top-level await works (e.g. `await client.get(url)`); the session keeps one event loop, so async clients and pools created in one call stay usable in later calls. Times out after 60s by default; pass `timeout_secs` to allow longer. On timeout or a client cancel the call returns an error and the worker is restarted, so a hung cell can't wedge the session (session state is lost on restart)."
    )]
    async fn python_eval(
        &self,
        Parameters(request): Parameters<EvalRequest>,
        cancel: CancellationToken,
    ) -> CallToolResult {
        let timeout = call_timeout(request.timeout_secs, DEFAULT_TIMEOUT_SECS);
        self.call_content(
            request.session_id,
            "eval",
            &request.intent,
            json!({ "expression": request.expression }),
            timeout,
            cancel,
        )
        .await
    }

    #[tool(
        description = "Execute Python statements in a persistent session. Pass a one-line `intent` describing what the call is for; it titles the run's dashboard card. Top-level await works (e.g. `await pool.fetch(sql)`); the session keeps one event loop, so async resources created in one call stay usable in later calls. Times out after 60s by default; pass `timeout_secs` to allow longer. On timeout or a client cancel the call returns an error and the worker is restarted, so a hung cell can't wedge the session (session state is lost on restart)."
    )]
    async fn python_exec(
        &self,
        Parameters(request): Parameters<ExecRequest>,
        cancel: CancellationToken,
    ) -> CallToolResult {
        let timeout = call_timeout(request.timeout_secs, DEFAULT_TIMEOUT_SECS);
        self.call_content(
            request.session_id,
            "exec",
            &request.intent,
            json!({ "source": request.source }),
            timeout,
            cancel,
        )
        .await
    }

    #[tool(description = "Clear a persistent Python session.")]
    async fn python_reset(
        &self,
        Parameters(request): Parameters<OptionalSessionRequest>,
        cancel: CancellationToken,
    ) -> String {
        self.call_text(request.session_id, "reset", json!({}), CONTROL_TIMEOUT, cancel)
            .await
    }

    #[tool(
        description = "Read-only semantic search over the shared `index` corpus via the bundled `search` module: code plus Claude/Codex/shell history across the fleet. Does NOT index — the separate `indexer` populates the store, so a query never uploads your local checkout. Scope server-side with `source` (code, claude_history, codex, shell, slack, linear, web), `user`, `repo` (e.g. indexable-inc/index), `host`, `project`; with no selector the whole corpus is searched. Returns matching chunks as JSON. Needs a Mixedbread credential (MXBAI_API_KEY or a prior `mgrep login`)."
    )]
    async fn search_semantic(
        &self,
        Parameters(request): Parameters<SemanticSearchRequest>,
        cancel: CancellationToken,
    ) -> String {
        // Run the search inside the session's persistent event loop via the
        // bundled module: import it, await `semantic`, then emit JSON. Every
        // argument is interpolated as a JSON literal, which is a valid Python
        // literal too, so a query or scope value containing quotes or newlines
        // cannot break out of the expression.
        let scope = scope_kwargs(
            request.source.as_deref(),
            request.user.as_deref(),
            request.repo.as_deref(),
            request.host.as_deref(),
            request.project.as_deref(),
        );
        let source = format!(
            "import json, search\n\
             _ix_hits = await search.semantic({query}, top_k={top_k}{scope})\n\
             print(json.dumps(_ix_hits))\n",
            query = json!(request.query),
            top_k = request.top_k.unwrap_or(10),
        );
        let timeout = call_timeout(request.timeout_secs, SEARCH_TIMEOUT_SECS);
        self.call_text(request.session_id, "exec", json!({ "source": source }), timeout, cancel)
            .await
    }

    #[tool(
        description = "Read-only regex grep over the shared `index` corpus via the bundled `search` module: run a regular expression over the SAME corpus chunks the semantic search covers (code plus Claude/Codex/shell history across the fleet). Does NOT index — the separate `indexer` populates the store, so a query never uploads your local checkout. Scope server-side with `source`, `user`, `repo`, `host`, `project`; with no selector the whole corpus is searched. Returns matching chunks as JSON. Needs a Mixedbread credential (MXBAI_API_KEY or a prior `mgrep login`)."
    )]
    async fn search_grep(
        &self,
        Parameters(request): Parameters<GrepSearchRequest>,
        cancel: CancellationToken,
    ) -> String {
        // Run the grep inside the session's persistent event loop via the bundled
        // module: import it, await `grep`, then emit JSON. Every argument is
        // interpolated as a JSON literal, which is a valid Python literal too, so
        // content with quotes or newlines cannot break out of the expression.
        let scope = scope_kwargs(
            request.source.as_deref(),
            request.user.as_deref(),
            request.repo.as_deref(),
            request.host.as_deref(),
            request.project.as_deref(),
        );
        let source = format!(
            "import json, search\n\
             _ix_hits = await search.grep({pattern}, top_k={top_k}, case_sensitive={case_sensitive}{scope})\n\
             print(json.dumps(_ix_hits))\n",
            pattern = json!(request.pattern),
            top_k = request.top_k.unwrap_or(10),
            case_sensitive = if request.case_sensitive.unwrap_or(false) {
                "True"
            } else {
                "False"
            },
        );
        let timeout = call_timeout(request.timeout_secs, SEARCH_TIMEOUT_SECS);
        self.call_text(request.session_id, "exec", json!({ "source": source }), timeout, cancel)
            .await
    }
}

/// Build the Python keyword-argument fragment for the scope selectors, e.g.
/// `, source=["code"], user=["andrew"]`. Each value is emitted as a JSON literal
/// (a valid Python literal), and only provided, non-empty selectors are included,
/// so an omitted filter leaves the corpus unscoped on that axis. The leading `, `
/// lets the caller append it directly after the fixed positional arguments.
fn scope_kwargs(
    source: Option<&[String]>,
    user: Option<&[String]>,
    repo: Option<&str>,
    host: Option<&[String]>,
    project: Option<&[String]>,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    for (key, values) in [
        ("source", source),
        ("user", user),
        ("host", host),
        ("project", project),
    ] {
        if let Some(values) = values.filter(|values| !values.is_empty()) {
            let _ = write!(out, ", {key}={}", json!(values));
        }
    }
    if let Some(repo) = repo.filter(|repo| !repo.is_empty()) {
        let _ = write!(out, ", repo={}", json!(repo));
    }
    out
}

impl McpServer {
    /// Run a registry-only operation (create/list/close) off the async runtime.
    /// Session creation builds a venv, which is slow and must not block a runtime
    /// worker thread, so even the cheap ops take the blocking pool for one
    /// uniform path. The registry lock is held only for the duration of `f`.
    async fn with_registry<F>(&self, f: F) -> String
    where
        F: FnOnce(&mut SessionManager) -> Result<String> + Send + 'static,
    {
        let sessions = self.sessions.clone();
        let joined = tokio::task::spawn_blocking(move || match sessions.lock() {
            Ok(mut registry) => f(&mut registry),
            Err(error) => Err(anyhow!("Python session registry lock failed: {error}")),
        })
        .await;
        match joined {
            Ok(result) => format_result(result),
            Err(error) => format!("stderr:\nPython registry task panicked: {error}"),
        }
    }

    /// Run a per-session round-trip off the async runtime. The registry lock is
    /// taken only to look up (or create) this session's handle, then released, so
    /// a long or hung call on one session never blocks calls to another session.
    /// (The one exception is creating a brand-new session: that builds a venv
    /// while holding the registry lock, which briefly serializes other sessions'
    /// first call and the management tools. Existing sessions are unaffected.)
    /// The round-trip itself runs on the blocking pool with the supplied deadline
    /// and the request's cancellation token.
    async fn run_on_session(
        &self,
        session_id: Option<String>,
        op: &'static str,
        payload: Value,
        timeout: Duration,
        cancel: CancellationToken,
        on_partial: Option<PartialSink>,
    ) -> Result<Value> {
        let sessions = self.sessions.clone();
        tokio::task::spawn_blocking(move || -> Result<Value> {
            let session = {
                let mut registry = sessions
                    .lock()
                    .map_err(|error| anyhow!("Python session registry lock failed: {error}"))?;
                registry.get_or_create_arc(session_id.as_deref())?
            };
            let mut session = session
                .lock()
                .map_err(|error| anyhow!("Python session lock failed: {error}"))?;
            // A `None` handler means this call does not stream (control ops,
            // search); a no-op stands in so `request_raw` always has a sink.
            let mut handler = on_partial;
            let mut noop = |_: &str| {};
            let partial: &mut dyn FnMut(&str) = match handler.as_mut() {
                Some(boxed) => boxed.as_mut(),
                None => &mut noop,
            };
            session.request_raw(op, payload, timeout, &cancel, partial)
        })
        .await
        .context("Python worker task panicked")?
    }

    /// Per-session call that returns rich content: the formatted text plus an
    /// image block per captured figure, so a `plt.plot`, PIL image, or
    /// `display()`ed object comes back as an actual image.
    ///
    /// Each call is also mirrored to the dashboard as an `exec` pane: a running
    /// card when the call starts, replaced with its captured output when it
    /// returns, so every execution is visible and replayable.
    async fn call_content(
        &self,
        session_id: Option<String>,
        op: &'static str,
        intent: &str,
        payload: Value,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> CallToolResult {
        let exec_id = self.board.map(|board| {
            let session = session_id.as_deref().unwrap_or(DEFAULT_SESSION_ID);
            board.start(session, "python", op, intent, exec_source(op, &payload))
        });

        // While the call runs, stream each stdout chunk into its dashboard pane
        // so output is visible live, not only when the call returns.
        let on_partial: Option<PartialSink> = match (self.board, exec_id.clone()) {
            (Some(board), Some(id)) => Some(Box::new(move |chunk: &str| board.append(&id, chunk))),
            _ => None,
        };

        let outcome = self
            .run_on_session(session_id, op, payload, timeout, cancel, on_partial)
            .await;

        if let (Some(board), Some(id)) = (self.board, &exec_id) {
            match &outcome {
                Ok(response) => board.finish_from_response(id, response),
                Err(error) => board.finish_error(id, &format!("{error:#}")),
            }
        }

        match outcome {
            Ok(response) => worker_response_content(&response),
            Err(error) => CallToolResult::success(vec![Content::text(format!("stderr:\n{error:#}"))]),
        }
    }

    /// Per-session call that returns plain text (reset, search).
    async fn call_text(
        &self,
        session_id: Option<String>,
        op: &'static str,
        payload: Value,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> String {
        match self.run_on_session(session_id, op, payload, timeout, cancel, None).await {
            Ok(response) => format_worker_response(&response),
            Err(error) => format!("stderr:\n{error:#}"),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateSessionRequest {
    cwd: Option<PathBuf>,
    session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OptionalSessionRequest {
    session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SessionRequest {
    session_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EvalRequest {
    /// One short, human-readable line stating what this call is for (e.g. "count
    /// rows in the orders parquet"). Shown as the run's dashboard card title so
    /// the board reads as a list of intents, not raw code. Required.
    intent: String,
    expression: String,
    session_id: Option<String>,
    /// Seconds to wait before abandoning the call and restarting the worker.
    /// Defaults to 60; raise it for long-running work. `0` uses the default.
    timeout_secs: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecRequest {
    /// One short, human-readable line stating what this call is for (e.g. "load
    /// the CSV and build the join index"). Shown as the run's dashboard card
    /// title so the board reads as a list of intents, not raw code. Required.
    intent: String,
    source: String,
    session_id: Option<String>,
    /// Seconds to wait before abandoning the call and restarting the worker.
    /// Defaults to 60; raise it for long-running work. `0` uses the default.
    timeout_secs: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SemanticSearchRequest {
    /// Natural-language query to search the corpus for.
    query: String,
    /// Maximum number of results to return (default 10).
    top_k: Option<usize>,
    /// Restrict to these sources: `code`, `claude_history`, `codex`, `shell`,
    /// `slack`, `linear`, `web`. Omit to search every source.
    source: Option<Vec<String>>,
    /// Restrict to records authored by these users. Omit for every user.
    user: Option<Vec<String>>,
    /// Restrict code to this repository slug, e.g. indexable-inc/index. Omit for
    /// every repository.
    repo: Option<String>,
    /// Restrict to records recorded on these hosts. Omit for every host.
    host: Option<Vec<String>>,
    /// Restrict to these project slugs (e.g. a Claude transcript's project
    /// directory). Omit for every project.
    project: Option<Vec<String>>,
    session_id: Option<String>,
    /// Seconds to wait before abandoning the search. Defaults to 600. `0` uses
    /// the default.
    timeout_secs: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GrepSearchRequest {
    /// Regular expression to match against the corpus chunks.
    pattern: String,
    /// Maximum number of results to return (default 10).
    top_k: Option<usize>,
    /// Match the pattern case-sensitively (default false).
    case_sensitive: Option<bool>,
    /// Restrict to these sources: `code`, `claude_history`, `codex`, `shell`,
    /// `slack`, `linear`, `web`. Omit to search every source.
    source: Option<Vec<String>>,
    /// Restrict to records authored by these users. Omit for every user.
    user: Option<Vec<String>>,
    /// Restrict code to this repository slug, e.g. indexable-inc/index. Omit for
    /// every repository.
    repo: Option<String>,
    /// Restrict to records recorded on these hosts. Omit for every host.
    host: Option<Vec<String>>,
    /// Restrict to these project slugs. Omit for every project.
    project: Option<Vec<String>>,
    session_id: Option<String>,
    /// Seconds to wait before abandoning the search. Defaults to 600. `0` uses
    /// the default.
    timeout_secs: Option<u64>,
}

#[derive(Default)]
struct SessionManager {
    sessions: HashMap<String, Arc<Mutex<PythonSession>>>,
}

impl SessionManager {
    /// Build a new session and register it. Building runs the venv create and a
    /// readiness ping, so this holds the registry lock for the build duration:
    /// concurrent first-calls to *other* new sessions wait behind it. That cost
    /// is one-time per session and small relative to the work itself.
    fn create_arc(
        &mut self,
        session_id: Option<String>,
        cwd: Option<PathBuf>,
    ) -> Result<Arc<Mutex<PythonSession>>> {
        let id = session_id.unwrap_or_else(|| uuid_like(&self.sessions));
        if self.sessions.contains_key(&id) {
            bail!("Python session {id} already exists");
        }
        let session = PythonSession::start(id.clone(), cwd)?;
        let arc = Arc::new(Mutex::new(session));
        self.sessions.insert(id, arc.clone());
        Ok(arc)
    }

    fn get_or_create_arc(&mut self, session_id: Option<&str>) -> Result<Arc<Mutex<PythonSession>>> {
        let id = session_id.unwrap_or(DEFAULT_SESSION_ID);
        if let Some(existing) = self.sessions.get(id) {
            return Ok(existing.clone());
        }
        self.create_arc(Some(id.to_string()), None)
    }

    fn close(&mut self, session_id: &str) -> Result<String> {
        let Some(arc) = self.sessions.remove(session_id) else {
            bail!("Python session {session_id} does not exist");
        };
        match arc.try_lock() {
            Ok(mut session) => {
                session.close_worker();
                Ok(format!("closed Python session {session_id}"))
            }
            // An in-flight call holds the lock. Removing it from the registry
            // means no new call can reach it; the worker is closed when that call
            // finishes and the last handle drops.
            Err(TryLockError::WouldBlock) => Ok(format!(
                "Python session {session_id} is busy; detached, it closes when the in-flight call finishes"
            )),
            Err(TryLockError::Poisoned(_)) => Ok(format!(
                "Python session {session_id} was poisoned by a panic; dropped"
            )),
        }
    }

    fn list(&mut self) -> Result<String> {
        if self.sessions.is_empty() {
            return Ok("no Python sessions".to_string());
        }
        let mut rows: Vec<SessionRow> = self
            .sessions
            .iter()
            .map(|(id, arc)| {
                arc.try_lock().map_or_else(
                    // Locked means a call is in flight: by definition running. We
                    // cannot read the command/cwd without the lock, so leave them
                    // empty rather than block the listing on a long call.
                    |_| SessionRow {
                        id: id.clone(),
                        command: Vec::new(),
                        cwd: None,
                        running: true,
                    },
                    |mut session| SessionRow {
                        id: id.clone(),
                        command: session.command.clone(),
                        cwd: session.cwd.clone(),
                        running: session.conn.child.try_wait().is_ok_and(|status| status.is_none()),
                    },
                )
            })
            .collect();
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        serde_json::to_string_pretty(&rows).context("failed to serialize session list")
    }
}

struct PythonSession {
    id: String,
    command: Vec<String>,
    env: Vec<(OsString, OsString)>,
    cwd: Option<PathBuf>,
    worker_path: PathBuf,
    // The venv and worker script live inside this directory, so it must outlive
    // the worker (including across a restart, which reuses the same venv).
    _temp_dir: TempDir,
    conn: WorkerConn,
}

impl PythonSession {
    /// Start a session on the pinned interpreter. Each session gets its own
    /// writable venv, activated for the worker and the children it spawns, so an
    /// agent can `pip install` into it without mutating the read-only store
    /// interpreter.
    fn start(id: String, cwd: Option<PathBuf>) -> Result<Self> {
        let temp_dir = session_temp_dir(&id)?;
        let venv_dir = create_venv(temp_dir.path())?;
        let python = venv_dir.join("bin/python").display().to_string();
        let env = venv_env(&venv_dir)?;
        Self::spawn(id, temp_dir, vec![python], cwd, env)
    }

    /// Spawn a worker against an explicit command, skipping the venv build and
    /// activation. Tests use this to inject a bare interpreter or a fake worker;
    /// production always pins the interpreter through `start`.
    #[cfg(test)]
    fn spawn_command(id: String, command: Vec<String>, cwd: Option<PathBuf>) -> Result<Self> {
        let temp_dir = session_temp_dir(&id)?;
        Self::spawn(id, temp_dir, command, cwd, Vec::new())
    }

    fn spawn(
        id: String,
        temp_dir: TempDir,
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: Vec<(OsString, OsString)>,
    ) -> Result<Self> {
        let worker_path = temp_dir.path().join("worker.py");
        fs::write(&worker_path, WORKER_SOURCE).context("failed to write Python worker")?;
        let conn = WorkerConn::spawn(&command, &env, &worker_path, cwd.as_deref())?;
        Ok(Self {
            id,
            command,
            env,
            cwd,
            worker_path,
            _temp_dir: temp_dir,
            conn,
        })
    }

    /// Send a request and wait for the matching response, honouring `timeout` and
    /// `cancel`. Any transport-level failure (timeout, client cancel, the worker
    /// exiting, a protocol desync, or a broken pipe) replaces the worker with a
    /// fresh one before returning the error, so the *next* call lands on a
    /// working interpreter instead of inheriting the wedged one. A normal Python
    /// exception is not a transport failure: it comes back as a successful
    /// response with `ok: false`, and the worker is left alone.
    fn request_raw(
        &mut self,
        op: &str,
        payload: Value,
        timeout: Duration,
        cancel: &CancellationToken,
        on_partial: &mut dyn FnMut(&str),
    ) -> Result<Value> {
        match self.conn.roundtrip(op, payload, timeout, cancel, on_partial) {
            Ok(value) => Ok(value),
            Err(error) => {
                let error = error.context(format!("Python session {}", self.id));
                match self.recover() {
                    Ok(()) => Err(error),
                    Err(restart) => {
                        Err(error.context(format!("and restarting the worker failed: {restart:#}")))
                    }
                }
            }
        }
    }

    /// Synchronous round-trip with no client cancellation, for CLI calls and
    /// internal control ops. Formats the worker response like the tools do.
    fn request(&mut self, op: &str, payload: Value, timeout: Duration) -> Result<String> {
        Ok(format_worker_response(&self.request_raw(
            op,
            payload,
            timeout,
            &uncancellable(),
            &mut |_: &str| {},
        )?))
    }

    /// Replace the worker with a fresh one against the same venv. Reuses the
    /// stored command, env, and worker script, so this does not rebuild the venv;
    /// it just respawns the interpreter.
    fn recover(&mut self) -> Result<()> {
        // Kill the wedged worker before spawning the replacement. This frees its
        // resources first (lower peak memory, so the respawn is less likely to
        // fail under pressure) and stops it producing more stale responses. If
        // the respawn then fails, `self.conn` is the *dead* worker, so the next
        // call's write fails fast and re-enters recovery instead of reading a
        // stale buffered response off a still-running old worker.
        self.conn.kill();
        let fresh = WorkerConn::spawn(&self.command, &self.env, &self.worker_path, self.cwd.as_deref())
            .context("failed to restart Python worker")?;
        // Dropping the old connection joins its reader thread.
        drop(std::mem::replace(&mut self.conn, fresh));
        Ok(())
    }

    /// Ask the worker to exit cleanly, then reap it. Used on session close and
    /// drop. A worker that does not answer the graceful `close` in time is left
    /// for the connection's Drop to kill.
    fn close_worker(&mut self) {
        if self.conn.child.try_wait().ok().flatten().is_none() {
            let _ = self.conn.roundtrip("close", json!({}), CONTROL_TIMEOUT, &uncancellable(), &mut |_: &str| {});
            let _ = self.conn.child.wait();
        }
    }
}

impl Drop for PythonSession {
    fn drop(&mut self) {
        self.close_worker();
        // The connection's own Drop then kills the process if it is still alive
        // and joins the reader thread.
    }
}

/// A live worker process plus the machinery to talk to it: its stdin, a
/// background thread draining stdout into a channel, and the next request id.
struct WorkerConn {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<String>,
    reader: Option<JoinHandle<()>>,
    next_id: u64,
}

impl WorkerConn {
    fn spawn(
        command: &[String],
        env: &[(OsString, OsString)],
        worker_path: &Path,
        cwd: Option<&Path>,
    ) -> Result<Self> {
        if command.is_empty() {
            bail!("Python session command must not be empty");
        }
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .arg(worker_path)
            .current_dir(cwd.unwrap_or_else(|| Path::new(".")))
            .envs(env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Worker stderr is not part of the JSON protocol. Normal Python
            // stderr is captured in-process; raw fd 2 writes must not back up and
            // block stdout responses.
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start Python command {}", command.join(" ")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Python worker is missing stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Python worker is missing stdout"))?;

        // Drain stdout on a dedicated thread so the protocol read can wait with a
        // deadline (std's BufRead has no timeout). Each response line is
        // forwarded over the channel; when the worker exits or is killed, the
        // read hits EOF, the thread ends, and the channel disconnects, which the
        // waiter observes promptly.
        let (tx, rx) = mpsc::channel::<String>();
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if tx.send(std::mem::take(&mut line)).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut conn = Self {
            child,
            stdin,
            rx,
            reader: Some(reader),
            next_id: 0,
        };
        // Confirm the worker's read loop is live before handing the session out.
        conn.roundtrip("ping", json!({}), CONTROL_TIMEOUT, &uncancellable(), &mut |_: &str| {})
            .context("Python worker did not become ready")?;
        Ok(conn)
    }

    fn roundtrip(
        &mut self,
        op: &str,
        mut payload: Value,
        timeout: Duration,
        cancel: &CancellationToken,
        on_partial: &mut dyn FnMut(&str),
    ) -> Result<Value> {
        let request_id = self.next_id;
        self.next_id += 1;
        let request = payload
            .as_object_mut()
            .ok_or_else(|| anyhow!("Python worker request payload must be an object"))?;
        request.insert("id".to_string(), json!(request_id));
        request.insert("op".to_string(), json!(op));

        writeln!(self.stdin, "{}", Value::Object(request.clone()))
            .context("failed to write Python worker request")?;
        self.stdin
            .flush()
            .context("failed to flush Python worker request")?;

        // `Instant + Duration` panics on overflow, so cap an out-of-range budget
        // at MAX_TIMEOUT instead. Callers route through `call_timeout`, which
        // already clamps; this guards any direct caller (and the fixed control
        // timeouts, which are always small).
        let now = Instant::now();
        let deadline = now.checked_add(timeout).unwrap_or_else(|| now + MAX_TIMEOUT);
        loop {
            if cancel.is_cancelled() {
                bail!(
                    "Python call cancelled by the client; the worker was restarted, so session state was lost"
                );
            }
            // Wake at least every POLL_INTERVAL to re-check cancellation, even
            // when the deadline is far off.
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(remaining.min(POLL_INTERVAL)) {
                Ok(line) => {
                    let response: Value = serde_json::from_str(&line)
                        .context("failed to decode Python worker response")?;
                    let id_matches = response.get("id").and_then(Value::as_u64) == Some(request_id);
                    // A `partial` message streams interim stdout for this call and
                    // never terminates the wait; only the final response (no
                    // `partial` flag) returns. A partial for another id is ignored
                    // defensively (calls on one session are serial, so it should
                    // not happen).
                    if response.get("partial").and_then(Value::as_bool) == Some(true) {
                        if id_matches
                            && let Some(chunk) = response.get("stdout").and_then(Value::as_str)
                        {
                            on_partial(chunk);
                        }
                        continue;
                    }
                    if !id_matches {
                        bail!("Python worker returned a response for the wrong request");
                    }
                    return Ok(response);
                }
                Err(RecvTimeoutError::Timeout) => {
                    if cancel.is_cancelled() {
                        bail!(
                            "Python call cancelled by the client; the worker was restarted, so session state was lost"
                        );
                    }
                    if Instant::now() >= deadline {
                        bail!(
                            "Python call exceeded its {}s timeout and was abandoned; the worker was restarted (session state lost). Pass a larger `timeout_secs` for long-running work.",
                            timeout.as_secs()
                        );
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    bail!("Python worker exited before responding; it was restarted");
                }
            }
        }
    }

    /// Terminate the worker process and reap it. Idempotent: a second call after
    /// the child has exited is a no-op. The reader thread observes the closed
    /// stdout (EOF) and exits; it is joined when the connection is dropped.
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for WorkerConn {
    fn drop(&mut self) {
        // Best-effort: make sure the process is gone and the reader thread is
        // joined, so a dropped connection never leaks a Python process or a
        // thread. Killing closes stdout, which lets the reader hit EOF and exit.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[derive(Serialize)]
struct SessionRow {
    id: String,
    command: Vec<String>,
    cwd: Option<PathBuf>,
    running: bool,
}

fn session_temp_dir(id: &str) -> Result<TempDir> {
    tempfile::Builder::new()
        .prefix(&format!("ix-mcp-python-{id}-"))
        .tempdir()
        .context("failed to create Python session directory")
}

/// Build a writable venv from the pinned interpreter and return its directory.
/// The interpreter is fixed at `IX_MCP_PYTHON`, which the Nix wrapper sets. A
/// missing pin is a hard error rather than an ambient-`python3` fallback, so a
/// wrapper regression fails loudly instead of silently running whatever is on
/// `PATH`. The venv gives each session a writable site-packages with no
/// per-call interpreter choice.
fn create_venv(temp_dir: &Path) -> Result<PathBuf> {
    let python = std::env::var("IX_MCP_PYTHON").context(
        "IX_MCP_PYTHON is unset; run ix-mcp via its Nix wrapper, which pins the interpreter",
    )?;
    let venv_dir = temp_dir.join(".venv");
    // `--system-site-packages` exposes the pinned interpreter's bundled
    // packages (notably `tui`) to the session while keeping the venv writable,
    // so an in-session `pip install` still lands in the per-session venv.
    let status = Command::new(&python)
        .args(["-m", "venv", "--system-site-packages"])
        .arg(&venv_dir)
        .status()
        .with_context(|| format!("failed to create Python environment with {python}"))?;
    if !status.success() {
        bail!("Python environment command exited with {status}");
    }
    Ok(venv_dir)
}

/// Activation environment for a child running in `venv_dir`: point
/// `VIRTUAL_ENV` at the venv and prepend its `bin` to `PATH`. Running the
/// venv's own `python` already resolves the venv site-packages, but a child the
/// session spawns (an in-session `pip`, or any `subprocess` that finds a tool
/// by name) would otherwise hit the host `PATH`. This makes the writable-venv
/// promise hold for those too.
fn venv_env(venv_dir: &Path) -> Result<Vec<(OsString, OsString)>> {
    let bin = venv_dir.join("bin");
    let path = match std::env::var_os("PATH") {
        Some(existing) => {
            let mut entries = vec![bin];
            entries.extend(std::env::split_paths(&existing));
            std::env::join_paths(entries).context("failed to compose venv PATH")?
        }
        None => bin.into_os_string(),
    };
    Ok(vec![
        ("VIRTUAL_ENV".into(), venv_dir.as_os_str().to_owned()),
        ("PATH".into(), path),
    ])
}

/// Cap on image blocks per response, so a cell that opens dozens of figures
/// cannot balloon a single tool result. The worker also caps; this is a
/// defense-in-depth ceiling on the rendering side.
const MAX_IMAGES: usize = 8;

/// Build tool content from a worker response: the formatted text first, then an
/// image block for each captured rich image (`{ "mime", "base64" }` entries in
/// the worker's `images` field).
fn worker_response_content(response: &Value) -> CallToolResult {
    let mut content = vec![Content::text(format_worker_response(response))];
    if let Some(images) = response.get("images").and_then(Value::as_array) {
        for image in images.iter().take(MAX_IMAGES) {
            if let (Some(mime), Some(data)) = (
                image.get("mime").and_then(Value::as_str),
                image.get("base64").and_then(Value::as_str),
            ) {
                content.push(Content::image(data.to_owned(), mime.to_owned()));
            }
        }
    }
    CallToolResult::success(content)
}

/// The source to show on an execution's dashboard pane: the expression for an
/// `eval`, the statements for an `exec`. Falls back to the op name so a pane
/// always has something behind it.
fn exec_source(op: &str, payload: &Value) -> String {
    let key = match op {
        "eval" => "expression",
        _ => "source",
    };
    payload
        .get(key)
        .and_then(Value::as_str)
        .map_or_else(|| op.to_owned(), str::to_owned)
}

fn format_worker_response(response: &Value) -> String {
    let mut sections = Vec::new();
    if let Some(stdout) = response.get("stdout").and_then(Value::as_str)
        && !stdout.is_empty()
    {
        sections.push(format!("stdout:\n{}", stdout.trim_end()));
    }
    if let Some(stderr) = response.get("stderr").and_then(Value::as_str)
        && !stderr.is_empty()
    {
        sections.push(format!("stderr:\n{}", stderr.trim_end()));
    }
    if let Some(result) = response.get("result").and_then(Value::as_str)
        && !result.is_empty()
    {
        sections.push(format!("result:\n{result}"));
    }
    if sections.is_empty() {
        "ok".to_string()
    } else {
        sections.join("\n\n")
    }
}

fn format_result(result: Result<String>) -> String {
    match result {
        Ok(output) => output,
        Err(error) => format!("stderr:\n{error:#}"),
    }
}

fn uuid_like(sessions: &HashMap<String, Arc<Mutex<PythonSession>>>) -> String {
    for index in 1.. {
        let id = format!("python-{index}");
        if !sessions.contains_key(&id) {
            return id;
        }
    }
    unreachable!("unbounded session id search exhausted")
}

/// Serve over Streamable HTTP, the spec's HTTP transport (POST for requests,
/// SSE for streamed responses), at `addr`. Each MCP session gets a fresh
/// [`McpServer`] from the factory, so its Python sessions are owned by that
/// session and torn down when it ends, mirroring the one-owner lifecycle stdio
/// already has. The cancellation token tied to ctrl-c terminates in-flight SSE
/// sessions on shutdown rather than leaving them hanging.
async fn serve_http(addr: &str) -> Result<()> {
    let cancel = tokio_util::sync::CancellationToken::new();
    let service = StreamableHttpService::new(
        || Ok(McpServer::new()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_cancellation_token(cancel.child_token()),
    );

    let router = axum::Router::new().nest_service(HTTP_MCP_PATH, service);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind Streamable HTTP listener on {addr}"))?;
    eprintln!("ix-mcp Streamable HTTP listening on http://{addr}{HTTP_MCP_PATH}");

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            cancel.cancel();
        })
        .await
        .context("Streamable HTTP server error")?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(CliCommand::Serve { http: None }) {
        CliCommand::Serve { http: Some(addr) } => serve_http(&addr).await?,
        CliCommand::Serve { http: None } => {
            let service = McpServer::new().serve(stdio()).await?;
            service.waiting().await?;
        }
        CliCommand::Repl => {
            let temp_dir = tempfile::Builder::new()
                .prefix("ix-mcp-python-repl-")
                .tempdir()
                .context("failed to create Python REPL directory")?;
            // The venv lives inside temp_dir, so keep temp_dir bound until the
            // interactive process exits.
            let venv_dir = create_venv(temp_dir.path())?;
            let venv_python = venv_dir.join("bin/python");
            let status = Command::new(&venv_python)
                .arg("-i")
                .current_dir(cli.cwd.as_deref().unwrap_or_else(|| Path::new(".")))
                .envs(venv_env(&venv_dir)?)
                .status()
                .with_context(|| {
                    format!(
                        "failed to start Python REPL command {}",
                        venv_python.display()
                    )
                })?;
            let code = status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1);
            return Ok(ExitCode::from(code));
        }
        CliCommand::Eval { expression } => {
            let mut manager = SessionManager::default();
            let session = manager.create_arc(Some(DEFAULT_SESSION_ID.to_string()), cli.cwd)?;
            // Compute the output and release the session lock before printing.
            let output = session
                .lock()
                .map_err(|error| anyhow!("Python session lock failed: {error}"))?
                .request(
                    "eval",
                    json!({ "expression": expression }),
                    call_timeout(None, DEFAULT_TIMEOUT_SECS),
                )?;
            println!("{output}");
        }
        CliCommand::Exec { source } => {
            let mut manager = SessionManager::default();
            let session = manager.create_arc(Some(DEFAULT_SESSION_ID.to_string()), cli.cwd)?;
            // Compute the output and release the session lock before printing.
            let output = session
                .lock()
                .map_err(|error| anyhow!("Python session lock failed: {error}"))?
                .request(
                    "exec",
                    json!({ "source": source }),
                    call_timeout(None, DEFAULT_TIMEOUT_SECS),
                )?;
            println!("{output}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use std::{sync::mpsc, thread, time::Duration};

    use super::*;

    #[test]
    fn worker_stderr_burst_cannot_block_protocol() -> Result<()> {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(run_worker_stderr_burst_session());
        });

        let output = receiver
            .recv_timeout(Duration::from_secs(5))
            .context("worker stderr burst blocked the session protocol")??;
        assert_eq!(output, "result:\nok");
        Ok(())
    }

    fn python_session(id: &str) -> Result<PythonSession> {
        // The test derivation puts `python3` on PATH via
        // `packageTestInputs.ix-mcp` in lib/rust-workspace.nix. `spawn_command`
        // runs the worker directly, skipping the venv build the worker does not
        // need.
        PythonSession::spawn_command(id.to_string(), vec!["python3".to_string()], None)
    }

    #[test]
    fn top_level_await_persists_async_state_across_calls() -> Result<()> {
        let mut session = python_session("await-persist")?;

        // An async primitive created with top-level await in one call must stay
        // usable in the next call. A fresh asyncio.run() loop per call would
        // close the loop the queue is bound to before the second call runs, so
        // this guards the persistent-loop design rather than await syntax alone.
        let put = session.request(
            "exec",
            json!({ "source": "import asyncio\nq = asyncio.Queue()\nawait q.put(123)" }),
            Duration::from_secs(30),
        )?;
        assert_eq!(put, "ok");

        let got = session.request(
            "eval",
            json!({ "expression": "await q.get()" }),
            Duration::from_secs(30),
        )?;
        assert_eq!(got, "result:\n123");

        Ok(())
    }

    #[test]
    fn subprocess_output_is_captured_at_fd_level() -> Result<()> {
        // The dashboard must show a spawned process's output, not just Python
        // `print`. The worker redirects fds 1 and 2, so a `subprocess.run`
        // writing straight to stdout/stderr is captured in the response (and the
        // JSON-RPC channel, on a separate fd, stays intact).
        let mut session = python_session("subprocess-capture")?;
        let out = session.request(
            "exec",
            json!({
                "source": "import subprocess, sys\n\
                           subprocess.run([\"echo\", \"hi-stdout\"])\n\
                           subprocess.run([\"sh\", \"-c\", \"echo hi-stderr 1>&2\"])\n"
            }),
            Duration::from_secs(30),
        )?;
        assert!(out.contains("hi-stdout"), "subprocess stdout must be captured: {out}");
        assert!(out.contains("hi-stderr"), "subprocess stderr must be captured: {out}");

        // The session is still usable afterwards: the RPC protocol survived the
        // subprocess writing to the worker's real stdout.
        let after = session.request(
            "eval",
            json!({ "expression": "6 * 7" }),
            Duration::from_secs(30),
        )?;
        assert_eq!(after, "result:\n42");
        Ok(())
    }

    #[test]
    fn synchronous_expressions_still_evaluate() -> Result<()> {
        // Compiling with PyCF_ALLOW_TOP_LEVEL_AWAIT must not change plain code:
        // snippets without await run eagerly and return their value.
        let mut session = python_session("await-sync")?;
        let sum = session.request(
            "eval",
            json!({ "expression": "1 + 1" }),
            Duration::from_secs(30),
        )?;
        assert_eq!(sum, "result:\n2");
        Ok(())
    }

    #[test]
    fn hung_call_times_out_and_session_recovers() -> Result<()> {
        let mut session = python_session("timeout-recover")?;

        // A cell that never returns must not wedge the session. The call returns
        // an error near its budget instead of hanging, and the worker is
        // restarted, so the next call works on a fresh interpreter.
        let started = Instant::now();
        let timed_out = session.request_raw(
            "exec",
            json!({ "source": "while True:\n    pass" }),
            Duration::from_millis(500),
            &uncancellable(),
            &mut |_: &str| {},
        );
        assert!(timed_out.is_err(), "a never-returning cell must report an error");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the timeout must fire near its budget, not hang"
        );

        // The restarted worker is a fresh interpreter (globals from before the
        // timeout are gone), but the session is usable again.
        let after = session.request(
            "eval",
            json!({ "expression": "21 * 2" }),
            Duration::from_secs(30),
        )?;
        assert_eq!(after, "result:\n42");
        Ok(())
    }

    #[test]
    fn cancelled_call_returns_and_session_recovers() -> Result<()> {
        let mut session = python_session("cancel-recover")?;

        // A pre-cancelled token stands in for a client `notifications/cancelled`
        // arriving while the call is in flight: the call must abandon promptly
        // and restart the worker rather than block on the never-returning cell.
        let cancel = CancellationToken::new();
        cancel.cancel();
        let started = Instant::now();
        let cancelled = session.request_raw(
            "exec",
            json!({ "source": "while True:\n    pass" }),
            Duration::from_mins(1),
            &cancel,
            &mut |_: &str| {},
        );
        assert!(cancelled.is_err(), "a cancelled call must report an error");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a cancel must not wait out the full timeout"
        );

        // The next call lands on the restarted worker and succeeds.
        let after = session.request(
            "eval",
            json!({ "expression": "1 + 1" }),
            Duration::from_secs(30),
        )?;
        assert_eq!(after, "result:\n2");
        Ok(())
    }

    #[test]
    fn call_timeout_clamps_absurd_budgets() {
        // A huge `timeout_secs` must clamp, not flow into `Instant + Duration`,
        // which panics on overflow and (inside the held session lock) would
        // poison it and brick the session. Unset/0 fall back to the default.
        assert_eq!(call_timeout(Some(u64::MAX), DEFAULT_TIMEOUT_SECS), MAX_TIMEOUT);
        assert_eq!(call_timeout(None, 60), Duration::from_mins(1));
        assert_eq!(call_timeout(Some(0), 60), Duration::from_mins(1));
        assert_eq!(call_timeout(Some(30), 60), Duration::from_secs(30));
    }

    #[test]
    fn create_session_rejects_unknown_fields() {
        // A stale client that still sends the removed `command` field must fail
        // loudly at deserialization, not silently land on the pinned venv and
        // surface as a confusing import error later.
        let stale = json!({ "command": ["uv", "run", "python"], "cwd": "/tmp" });
        assert!(serde_json::from_value::<CreateSessionRequest>(stale).is_err());
    }

    fn run_worker_stderr_burst_session() -> Result<String> {
        let script = r#"
i=0
head -c 2097152 /dev/zero >&2
	while IFS= read -r _line; do
	  printf '{"id":%s,"ok":true,"stdout":"","stderr":"","result":"ok"}\n' "$i"
	  case "$_line" in
	    *'"op":"close"'*) break ;;
	  esac
	  i=$((i + 1))
	done
	"#;
        let command = vec![
            "sh".to_string(),
            "-c".to_string(),
            script.to_string(),
            "ix-mcp-fake-worker".to_string(),
        ];
        let mut session = PythonSession::spawn_command("stderr-burst".to_string(), command, None)?;
        session.request("eval", json!({ "expression": "unused" }), Duration::from_secs(5))
    }
}
