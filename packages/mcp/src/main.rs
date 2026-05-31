use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, ExitCode, Stdio},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::TempDir;

const DEFAULT_SESSION_ID: &str = "default";
const WORKER_SOURCE: &str = include_str!("python_worker.py");

// Surfaced to clients on initialize so an agent discovers the preinstalled
// packages without reading the build. `tui` is the bundled PTY driver; naming
// it here keeps one home for the fact instead of repeating it across every
// `python_*` tool description.
const SERVER_INSTRUCTIONS: &str = include_str!("server_instructions.txt");

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
    Serve,
    Repl,
    Eval { expression: String },
    Exec { source: String },
}

#[derive(Clone)]
struct McpServer {
    sessions: Arc<Mutex<SessionManager>>,
    tool_router: ToolRouter<Self>,
}

impl McpServer {
    fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(SessionManager::default())),
            tool_router: Self::tool_router(),
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
    fn python_session_create(
        &self,
        Parameters(request): Parameters<CreateSessionRequest>,
    ) -> String {
        self.with_sessions(|sessions| {
            sessions.create(request.session_id, request.cwd)?;
            Ok("session ready".to_string())
        })
    }

    #[tool(description = "List persistent Python sessions.")]
    fn python_session_list(&self) -> String {
        self.with_sessions(SessionManager::list)
    }

    #[tool(description = "Close a persistent Python session.")]
    fn python_session_close(&self, Parameters(request): Parameters<SessionRequest>) -> String {
        self.with_sessions(|sessions| sessions.close(&request.session_id))
    }

    #[tool(
        description = "Evaluate a Python expression in a persistent session. Top-level await works (e.g. `await client.get(url)`); the session keeps one event loop, so async clients and pools created in one call stay usable in later calls."
    )]
    fn python_eval(&self, Parameters(request): Parameters<EvalRequest>) -> CallToolResult {
        self.with_sessions_content(|sessions| {
            let session = sessions.get_or_create(request.session_id.as_deref())?;
            session.request_raw("eval", json!({ "expression": request.expression }))
        })
    }

    #[tool(
        description = "Execute Python statements in a persistent session. Top-level await works (e.g. `await pool.fetch(sql)`); the session keeps one event loop, so async resources created in one call stay usable in later calls."
    )]
    fn python_exec(&self, Parameters(request): Parameters<ExecRequest>) -> CallToolResult {
        self.with_sessions_content(|sessions| {
            let session = sessions.get_or_create(request.session_id.as_deref())?;
            session.request_raw("exec", json!({ "source": request.source }))
        })
    }

    #[tool(description = "Clear a persistent Python session.")]
    fn python_reset(&self, Parameters(request): Parameters<OptionalSessionRequest>) -> String {
        self.with_sessions(|sessions| {
            let session = sessions.get_or_create(request.session_id.as_deref())?;
            session.request("reset", json!({}))
        })
    }

    #[tool(
        description = "Semantic code search over a checkout via the bundled `search` module. Indexes new/changed files (content-addressed, deduplicated across worktrees), then returns matching chunks scoped to the checkout as JSON. Needs a Mixedbread credential (MXBAI_API_KEY or a prior `mgrep login`)."
    )]
    fn search_semantic(&self, Parameters(request): Parameters<SemanticSearchRequest>) -> String {
        self.with_sessions(|sessions| {
            let session = sessions.get_or_create(request.session_id.as_deref())?;
            // Run the search inside the session's persistent event loop via the
            // bundled module: import it, await `semantic`, then emit JSON. Both
            // arguments are interpolated as JSON literals, which are valid Python
            // literals too, so a query or path containing quotes or newlines
            // cannot break out of the expression.
            let source = format!(
                "import json, search\n\
                 _ix_hits = await search.semantic({query}, {path}, top_k={top_k})\n\
                 print(json.dumps(_ix_hits))\n",
                query = json!(request.query),
                path = json!(request.path.as_deref().unwrap_or(".")),
                top_k = request.top_k.unwrap_or(10),
            );
            session.request("exec", json!({ "source": source }))
        })
    }

    #[tool(
        description = "Regex grep over a checkout via the bundled `search` module: run a regular expression over the SAME indexed chunks the semantic search covers (content-addressed, deduplicated across worktrees), and return matching chunks scoped to the checkout as JSON. New/changed files are indexed first. Needs a Mixedbread credential (MXBAI_API_KEY or a prior `mgrep login`)."
    )]
    fn search_grep(&self, Parameters(request): Parameters<GrepSearchRequest>) -> String {
        self.with_sessions(|sessions| {
            let session = sessions.get_or_create(request.session_id.as_deref())?;
            // Run the grep inside the session's persistent event loop via the
            // bundled module: import it, await `grep`, then emit JSON. The
            // pattern and path are interpolated as JSON literals, which are valid
            // Python literals too, so content with quotes or newlines cannot
            // break out of the expression.
            let source = format!(
                "import json, search\n\
                 _ix_hits = await search.grep({pattern}, {path}, top_k={top_k}, case_sensitive={case_sensitive})\n\
                 print(json.dumps(_ix_hits))\n",
                pattern = json!(request.pattern),
                path = json!(request.path.as_deref().unwrap_or(".")),
                top_k = request.top_k.unwrap_or(10),
                case_sensitive = if request.case_sensitive.unwrap_or(false) {
                    "True"
                } else {
                    "False"
                },
            );
            session.request("exec", json!({ "source": source }))
        })
    }
}

impl McpServer {
    fn with_sessions(&self, f: impl FnOnce(&mut SessionManager) -> Result<String>) -> String {
        match self.sessions.lock() {
            Ok(mut sessions) => format_result(f(&mut sessions)),
            Err(error) => format!("stderr:\nPython session registry lock failed: {error}"),
        }
    }

    /// Like [`with_sessions`](Self::with_sessions) but returns rich content: the
    /// formatted text plus an image block per captured figure, so a `plt.plot`,
    /// PIL image, or `display()`ed object comes back as an actual image.
    fn with_sessions_content(
        &self,
        f: impl FnOnce(&mut SessionManager) -> Result<Value>,
    ) -> CallToolResult {
        match self.sessions.lock() {
            Ok(mut sessions) => match f(&mut sessions) {
                Ok(response) => worker_response_content(&response),
                Err(error) => {
                    CallToolResult::success(vec![Content::text(format!("stderr:\n{error:#}"))])
                }
            },
            Err(error) => CallToolResult::success(vec![Content::text(format!(
                "stderr:\nPython session registry lock failed: {error}"
            ))]),
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
    expression: String,
    session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecRequest {
    source: String,
    session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SemanticSearchRequest {
    /// Natural-language query to search the checkout for.
    query: String,
    /// Checkout directory to index and scope results to. Defaults to the
    /// session's working directory.
    path: Option<String>,
    /// Maximum number of results to return (default 10).
    top_k: Option<usize>,
    session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GrepSearchRequest {
    /// Regular expression to match against the indexed chunks.
    pattern: String,
    /// Checkout directory to index and scope results to. Defaults to the
    /// session's working directory.
    path: Option<String>,
    /// Maximum number of results to return (default 10).
    top_k: Option<usize>,
    /// Match the pattern case-sensitively (default false).
    case_sensitive: Option<bool>,
    session_id: Option<String>,
}

#[derive(Default)]
struct SessionManager {
    sessions: HashMap<String, PythonSession>,
}

impl SessionManager {
    fn create(
        &mut self,
        session_id: Option<String>,
        cwd: Option<PathBuf>,
    ) -> Result<&mut PythonSession> {
        let id = session_id.unwrap_or_else(|| uuid_like(&self.sessions));
        if self.sessions.contains_key(&id) {
            bail!("Python session {id} already exists");
        }
        let session = PythonSession::start(id.clone(), cwd)?;
        self.sessions.insert(id.clone(), session);
        self.sessions
            .get_mut(&id)
            .ok_or_else(|| anyhow!("Python session {id} disappeared after creation"))
    }

    fn get_or_create(&mut self, session_id: Option<&str>) -> Result<&mut PythonSession> {
        let id = session_id.unwrap_or(DEFAULT_SESSION_ID);
        if !self.sessions.contains_key(id) {
            self.create(Some(id.to_string()), None)?;
        }
        self.sessions
            .get_mut(id)
            .ok_or_else(|| anyhow!("Python session {id} does not exist"))
    }

    fn close(&mut self, session_id: &str) -> Result<String> {
        let Some(mut session) = self.sessions.remove(session_id) else {
            bail!("Python session {session_id} does not exist");
        };
        session.close();
        Ok(format!("closed Python session {session_id}"))
    }

    fn list(&mut self) -> Result<String> {
        if self.sessions.is_empty() {
            return Ok("no Python sessions".to_string());
        }
        let rows: Vec<SessionRow> = self
            .sessions
            .values_mut()
            .map(|session| SessionRow {
                id: session.id.clone(),
                command: session.command.clone(),
                cwd: session.cwd.clone(),
                running: session
                    .child
                    .try_wait()
                    .is_ok_and(|status| status.is_none()),
            })
            .collect();
        serde_json::to_string_pretty(&rows).context("failed to serialize session list")
    }
}

struct PythonSession {
    id: String,
    command: Vec<String>,
    cwd: Option<PathBuf>,
    _temp_dir: TempDir,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_request_id: u64,
}

impl PythonSession {
    /// Start a session on the pinned interpreter. Each session gets its own
    /// writable venv, activated for the worker and the children it spawns, so
    /// an agent can `pip install` into it without mutating the read-only store
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

        if command.is_empty() {
            bail!("Python session command must not be empty");
        }
        let mut child = Command::new(&command[0])
            .args(&command[1..])
            .arg(&worker_path)
            .current_dir(cwd.as_deref().unwrap_or_else(|| Path::new(".")))
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Worker stderr is not part of the JSON protocol. Normal Python
            // stderr is captured in-process; raw fd 2 writes must not back up
            // and block stdout responses.
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start Python command {}", command.join(" ")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("Python session {id} is missing stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("Python session {id} is missing stdout"))?;

        let mut session = Self {
            id,
            command,
            cwd,
            _temp_dir: temp_dir,
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_request_id: 0,
        };
        session.request("ping", json!({}))?;
        Ok(session)
    }

    fn request_raw(&mut self, op: &str, mut payload: Value) -> Result<Value> {
        let request_id = self.next_request_id;
        self.next_request_id += 1;
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

        let mut line = String::new();
        if self
            .stdout
            .read_line(&mut line)
            .context("failed to read Python worker response")?
            == 0
        {
            bail!("Python session {} exited before responding", self.id);
        }

        let response: Value =
            serde_json::from_str(&line).context("failed to decode Python worker response")?;
        if response.get("id").and_then(Value::as_u64) != Some(request_id) {
            bail!(
                "Python session {} returned response for the wrong request",
                self.id
            );
        }
        Ok(response)
    }

    fn request(&mut self, op: &str, payload: Value) -> Result<String> {
        Ok(format_worker_response(&self.request_raw(op, payload)?))
    }

    fn close(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.request("close", json!({}));
            let _ = self.child.wait();
        }
    }
}

impl Drop for PythonSession {
    fn drop(&mut self) {
        self.close();
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

fn uuid_like(sessions: &HashMap<String, PythonSession>) -> String {
    for index in 1.. {
        let id = format!("python-{index}");
        if !sessions.contains_key(&id) {
            return id;
        }
    }
    unreachable!("unbounded session id search exhausted")
}

#[tokio::main]
async fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(CliCommand::Serve) {
        CliCommand::Serve => {
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
            let session = manager.create(Some(DEFAULT_SESSION_ID.to_string()), cli.cwd)?;
            println!(
                "{}",
                session.request("eval", json!({ "expression": expression }))?
            );
        }
        CliCommand::Exec { source } => {
            let mut manager = SessionManager::default();
            let session = manager.create(Some(DEFAULT_SESSION_ID.to_string()), cli.cwd)?;
            println!("{}", session.request("exec", json!({ "source": source }))?);
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
        )?;
        assert_eq!(put, "ok");

        let got = session.request("eval", json!({ "expression": "await q.get()" }))?;
        assert_eq!(got, "result:\n123");

        Ok(())
    }

    #[test]
    fn synchronous_expressions_still_evaluate() -> Result<()> {
        // Compiling with PyCF_ALLOW_TOP_LEVEL_AWAIT must not change plain code:
        // snippets without await run eagerly and return their value.
        let mut session = python_session("await-sync")?;
        let sum = session.request("eval", json!({ "expression": "1 + 1" }))?;
        assert_eq!(sum, "result:\n2");
        Ok(())
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
        session.request("eval", json!({ "expression": "unused" }))
    }
}
