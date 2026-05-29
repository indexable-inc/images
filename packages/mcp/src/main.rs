use std::{
    collections::HashMap,
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
    model::{ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
    transport::stdio,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::TempDir;

const DEFAULT_SESSION_ID: &str = "default";
const WORKER_SOURCE: &str = include_str!("python_worker.py");

#[derive(Parser)]
#[command(name = "ix-mcp")]
struct Cli {
    #[arg(
        long = "python",
        global = true,
        allow_hyphen_values = true,
        help = "Python command segment for CLI eval/exec/repl. Repeat for arguments, e.g. --python uv --python run --python python."
    )]
    python: Vec<String>,

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
    }
}

#[tool_router]
impl McpServer {
    #[tool(description = "Create a persistent Python session with the chosen interpreter command.")]
    fn python_session_create(
        &self,
        Parameters(request): Parameters<CreateSessionRequest>,
    ) -> String {
        self.with_sessions(|sessions| {
            sessions.create(request.session_id, request.command, request.cwd)?;
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
    fn python_eval(&self, Parameters(request): Parameters<EvalRequest>) -> String {
        self.with_sessions(|sessions| {
            let session = sessions.get_or_create(request.session_id.as_deref())?;
            session.request("eval", json!({ "expression": request.expression }))
        })
    }

    #[tool(
        description = "Execute Python statements in a persistent session. Top-level await works (e.g. `await pool.fetch(sql)`); the session keeps one event loop, so async resources created in one call stay usable in later calls."
    )]
    fn python_exec(&self, Parameters(request): Parameters<ExecRequest>) -> String {
        self.with_sessions(|sessions| {
            let session = sessions.get_or_create(request.session_id.as_deref())?;
            session.request("exec", json!({ "source": request.source }))
        })
    }

    #[tool(description = "Clear a persistent Python session.")]
    fn python_reset(&self, Parameters(request): Parameters<OptionalSessionRequest>) -> String {
        self.with_sessions(|sessions| {
            let session = sessions.get_or_create(request.session_id.as_deref())?;
            session.request("reset", json!({}))
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
}

#[derive(Deserialize, JsonSchema)]
struct CreateSessionRequest {
    command: Option<Vec<String>>,
    cwd: Option<PathBuf>,
    session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct OptionalSessionRequest {
    session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct SessionRequest {
    session_id: String,
}

#[derive(Deserialize, JsonSchema)]
struct EvalRequest {
    expression: String,
    session_id: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ExecRequest {
    source: String,
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
        command: Option<Vec<String>>,
        cwd: Option<PathBuf>,
    ) -> Result<&mut PythonSession> {
        let id = session_id.unwrap_or_else(|| uuid_like(&self.sessions));
        if self.sessions.contains_key(&id) {
            bail!("Python session {id} already exists");
        }
        let session = PythonSession::start(id.clone(), command, cwd)?;
        self.sessions.insert(id.clone(), session);
        self.sessions
            .get_mut(&id)
            .ok_or_else(|| anyhow!("Python session {id} disappeared after creation"))
    }

    fn get_or_create(&mut self, session_id: Option<&str>) -> Result<&mut PythonSession> {
        let id = session_id.unwrap_or(DEFAULT_SESSION_ID);
        if !self.sessions.contains_key(id) {
            self.create(Some(id.to_string()), None, None)?;
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
    fn start(id: String, command: Option<Vec<String>>, cwd: Option<PathBuf>) -> Result<Self> {
        let temp_dir = tempfile::Builder::new()
            .prefix(&format!("ix-mcp-python-{id}-"))
            .tempdir()
            .context("failed to create Python session directory")?;
        let worker_path = temp_dir.path().join("worker.py");
        fs::write(&worker_path, WORKER_SOURCE).context("failed to write Python worker")?;

        let python_command = match command {
            Some(command) => command,
            None => create_default_environment(temp_dir.path())?,
        };
        if python_command.is_empty() {
            bail!("Python session command must not be empty");
        }

        let mut child = Command::new(&python_command[0])
            .args(&python_command[1..])
            .arg(&worker_path)
            .current_dir(cwd.as_deref().unwrap_or_else(|| Path::new(".")))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Worker stderr is not part of the JSON protocol. Normal Python
            // stderr is captured in-process; raw fd 2 writes must not back up
            // and block stdout responses.
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start Python command {}",
                    python_command.join(" ")
                )
            })?;
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
            command: python_command,
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

    fn request(&mut self, op: &str, mut payload: Value) -> Result<String> {
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
        Ok(format_worker_response(&response))
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

fn create_default_environment(temp_dir: &Path) -> Result<Vec<String>> {
    let python = std::env::var("IX_MCP_DEFAULT_PYTHON").unwrap_or_else(|_| "python3".to_string());
    let venv_dir = temp_dir.join(".venv");
    let status = Command::new(&python)
        .args(["-m", "venv"])
        .arg(&venv_dir)
        .status()
        .with_context(|| format!("failed to create default Python environment with {python}"))?;
    if !status.success() {
        bail!("default Python environment command exited with {status}");
    }
    Ok(vec![venv_dir.join("bin/python").display().to_string()])
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
            let (command, _temp_dir) = if cli.python.is_empty() {
                let temp_dir = tempfile::Builder::new()
                    .prefix("ix-mcp-python-repl-")
                    .tempdir()
                    .context("failed to create Python REPL directory")?;
                let command = create_default_environment(temp_dir.path())?;
                // The default command points inside the venv, so keep the temp
                // directory alive until the interactive process exits.
                (command, Some(temp_dir))
            } else {
                (cli.python, None)
            };
            let status = Command::new(&command[0])
                .args(&command[1..])
                .arg("-i")
                .current_dir(cli.cwd.as_deref().unwrap_or_else(|| Path::new(".")))
                .status()
                .with_context(|| {
                    format!("failed to start Python REPL command {}", command.join(" "))
                })?;
            let code = status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1);
            return Ok(ExitCode::from(code));
        }
        CliCommand::Eval { expression } => {
            let mut manager = SessionManager::default();
            let session = manager.create(
                Some(DEFAULT_SESSION_ID.to_string()),
                command_arg(cli.python),
                cli.cwd,
            )?;
            println!(
                "{}",
                session.request("eval", json!({ "expression": expression }))?
            );
        }
        CliCommand::Exec { source } => {
            let mut manager = SessionManager::default();
            let session = manager.create(
                Some(DEFAULT_SESSION_ID.to_string()),
                command_arg(cli.python),
                cli.cwd,
            )?;
            println!("{}", session.request("exec", json!({ "source": source }))?);
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn command_arg(command: Vec<String>) -> Option<Vec<String>> {
    if command.is_empty() {
        None
    } else {
        Some(command)
    }
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
        // `packageTestInputs.ix-mcp` in lib/rust-workspace.nix. A bare command
        // skips the default venv build, which the worker does not need.
        PythonSession::start(id.to_string(), Some(vec!["python3".to_string()]), None)
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
        let mut session = PythonSession::start("stderr-burst".to_string(), Some(command), None)?;
        session.request("eval", json!({ "expression": "unused" }))
    }
}
