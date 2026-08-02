//! Run a JSON-described DAG of commands and services with inline progress.
//!
//! The spec is a flat map of nodes, each with an argv `command`, an
//! optional `depends_on` list, an optional `env` overlay, and an
//! optional `timeout_secs` wall-clock limit. Nodes whose deps have
//! completed run as soon as they are unblocked, so the layout of the
//! graph determines how much parallelism is achievable; there is no
//! notion of "levels".
//!
//! A node is either a *task*, which runs to completion and is ready once it
//! exits zero, or a *service*, which stays up and is ready once its
//! `ready_when` probe passes. Dependents start on readiness rather than
//! completion, so a client waits for the port its server is about to open
//! instead of for a sleep. The run is over when the last task settles; any
//! service still up is then stopped as a group, and a spec with no tasks in
//! it runs until Ctrl-C, which is what supervising a set of services means.
//!
//! Output modes:
//! - `auto` (default): TUI on a TTY, plain otherwise.
//! - `tui`: indicatif `MultiProgress` with one inline spinner per node.
//! - `plain`: line-buffered "started" / "finished" lines, no spinners.
//! - `json`: NDJSON event stream plus a final `summary` record.
//!
//! Exit code reflects the worst node outcome: zero if every node succeeded or
//! was a service the runner stopped, the worst non-zero command exit code
//! otherwise, or 1 if any node was skipped. Ctrl-C cancels every running child
//! (SIGTERM, brief grace, SIGKILL) and forces exit 130; a second Ctrl-C
//! hard-exits immediately.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::io::{self, IsTerminal};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt as _;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use indicatif::{MultiProgress, ProgressBar};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::{Mutex, watch};

#[derive(Parser)]
#[command(
    about = "Run a JSON-described DAG of commands in parallel with inline progress.",
    version
)]
struct Args {
    /// Path to the JSON DAG spec.
    spec: PathBuf,

    /// Output mode.
    #[arg(long, value_enum, default_value_t = OutputMode::Auto)]
    output: OutputMode,

    /// Restrict the run to these node names. Comma-separated, repeatable
    /// (`--only a,b --only c`). Every name must exist in the spec, and any
    /// kept node may only depend on other kept nodes; otherwise the runner
    /// errors out before spawning anything so the remaining graph still has
    /// well-defined semantics (no silent skips, no exit-code surprises).
    #[arg(long, value_delimiter = ',')]
    only: Vec<String>,
}

#[derive(Clone, Copy, ValueEnum)]
enum OutputMode {
    Auto,
    Tui,
    Plain,
    Json,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    nodes: HashMap<String, NodeSpec>,
}

/// What the runner expects a node's process to do.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "snake_case")]
enum Kind {
    /// Runs to completion. Ready once it exits zero.
    #[default]
    Task,
    /// Stays up for the rest of the run. Ready once `ready_when` passes,
    /// stopped once every task has settled.
    Service,
}

/// Where a node's child process writes.
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "snake_case")]
enum StdioMode {
    /// Piped and retained for the failure dump; only the newest line is
    /// visible while it runs, on the spinner.
    #[default]
    Capture,
    /// Piped and retained, and each line also echoed as `name | line` as it
    /// arrives.
    Prefixed,
    /// The child writes straight to the runner's own stdout and stderr, so its
    /// terminal detection and colour survive. Nothing is captured, which means
    /// nothing is retained for the failure dump and `log_line` readiness has
    /// no stream to read.
    Inherit,
}

/// The condition that makes a service ready for its dependents to start.
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum ReadyWhen {
    Tcp(TcpReady),
    LogLine(LogLineReady),
    Http(HttpReady),
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
struct TcpReady {
    /// `host:port`, re-resolved on every attempt so a name that is not in DNS
    /// yet when the run starts still works once it is.
    address: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
struct LogLineReady {
    /// Matched as a substring, not a regex. A readiness banner is a fixed
    /// string, and a regex inside JSON is one more thing to escape wrong.
    pattern: String,
    #[serde(default)]
    stream: LogStream,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Default, Debug)]
#[serde(rename_all = "snake_case")]
enum LogStream {
    Stdout,
    Stderr,
    #[default]
    Either,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
struct HttpReady {
    url: String,
    /// Response status that counts as ready.
    #[serde(default = "default_ready_status")]
    status: u16,
    /// Substring the response body must contain. A port that answers is a
    /// weaker claim than a server that speaks the wire its caller parses, and
    /// the difference is what makes an unreadable window look like an empty
    /// one.
    #[serde(default)]
    body_contains: Option<String>,
}

const fn default_ready_status() -> u16 {
    200
}

const fn default_ready_timeout_secs() -> u64 {
    60
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
struct NodeSpec {
    /// `argv`. `command[0]` is the program; the rest are arguments.
    command: Vec<String>,
    #[serde(default)]
    depends_on: Vec<String>,
    /// Extra env vars layered on top of the runner's own environment.
    /// Parent env is inherited; entries here shadow it.
    #[serde(default)]
    env: BTreeMap<String, String>,
    /// Wall-clock seconds before the child is `SIGTERM`ed (then `SIGKILL`ed
    /// after a brief grace period). `None` means run to completion.
    /// Mirrors the `coreutils timeout` exit code on expiry: 124.
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    kind: Kind,
    /// Required on a service, rejected on a task: a task's readiness is its
    /// exit status, and there is nothing left to probe once it has one.
    #[serde(default)]
    ready_when: Option<ReadyWhen>,
    /// Wall-clock seconds a service gets to satisfy `ready_when`. Expiry is
    /// 124, for the same reason `timeout_secs` is.
    #[serde(default = "default_ready_timeout_secs")]
    ready_timeout_secs: u64,
    #[serde(default)]
    stdio: StdioMode,
    /// Hand the child the read end of a pipe on this descriptor and keep the
    /// write end for the life of the runner, so the child sees EOF the moment
    /// the runner is gone -- including when the runner is `SIGKILL`ed and runs
    /// no teardown at all.
    #[serde(default)]
    lifeline_fd: Option<i32>,
}

impl NodeSpec {
    const fn is_service(&self) -> bool {
        matches!(self.kind, Kind::Service)
    }
}

#[derive(Clone, Debug)]
enum Outcome {
    Succeeded,
    Failed(i32),
    Skipped,
    /// A service the runner took down itself. Not a failure: it stayed up for
    /// as long as anything needed it.
    Stopped,
}

impl Outcome {
    const fn label(&self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed(_) => "failed",
            Self::Skipped => "skipped",
            Self::Stopped => "stopped",
        }
    }

    /// What this outcome contributes to the runner's exit code.
    const fn exit_contribution(&self) -> i32 {
        match self {
            Self::Succeeded | Self::Stopped => 0,
            Self::Failed(code) => *code,
            Self::Skipped => 1,
        }
    }
}

/// The last `limit` lines a child wrote, plus a count of what fell off the
/// front. A service can run for hours, so keeping every line it ever emitted
/// would be an unbounded allocation on the exact path this exists to support.
#[derive(Default)]
struct OutputTail {
    lines: VecDeque<String>,
    dropped: usize,
    limit: usize,
}

impl OutputTail {
    const DEFAULT_LIMIT: usize = 500;

    fn new() -> Self {
        Self {
            lines: VecDeque::new(),
            dropped: 0,
            limit: Self::DEFAULT_LIMIT,
        }
    }

    fn push(&mut self, line: String) {
        self.lines.push_back(line);
        while self.lines.len() > self.limit.max(1) {
            self.lines.pop_front();
            self.dropped += 1;
        }
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// The retained text, headed by an explicit note when lines were dropped.
    /// A silently truncated dump is worse than a short one, because the reader
    /// cannot tell they are looking at part of the story.
    fn render(&self) -> String {
        let mut out = String::new();
        if self.dropped > 0 {
            let _ = writeln!(
                out,
                "dag-runner: {} earlier line{} dropped (keeping the last {})",
                self.dropped,
                if self.dropped == 1 { "" } else { "s" },
                self.limit
            );
        }
        for line in &self.lines {
            out.push_str(line);
        }
        out
    }
}

struct NodeRecord {
    outcome: Outcome,
    duration: Duration,
    stdout: OutputTail,
    stderr: OutputTail,
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event<'a> {
    NodeStarted {
        node: &'a str,
        ts_ms: u128,
    },
    /// A service satisfied its readiness probe. Always between that node's
    /// `node_started` and any dependent's `node_started`.
    NodeReady {
        node: &'a str,
        ts_ms: u128,
    },
    NodeFinished {
        node: &'a str,
        outcome: &'a str,
        exit_code: Option<i32>,
        duration_ms: u128,
    },
    Summary {
        total: usize,
        succeeded: usize,
        failed: usize,
        skipped: usize,
        stopped: usize,
        duration_ms: u128,
    },
}

enum CycleColor {
    Gray,
    Black,
}

/// Why the runner is taking the group down. One broadcast reaches every
/// running child, so "what stopped that process" has one answer per run
/// rather than one per code path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reason {
    /// The operator hit Ctrl-C. Forces exit 130 whatever the nodes did.
    Cancelled,
    /// Every task has settled, so a service still up has nothing left to
    /// serve. Not a failure.
    Drained,
    /// A service failed, so the rest of the group is running against
    /// something that is not there.
    Aborted,
}

impl Reason {
    /// The exit code for a *task* the runner had to terminate: `128 + signal`,
    /// which is what a shell reports for the same kill, so the number names
    /// the signal that actually landed.
    const fn terminated_exit_code(self) -> i32 {
        match self {
            Self::Cancelled => 130,
            Self::Drained | Self::Aborted => 143,
        }
    }

    const fn describe(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Drained => "run finished",
            Self::Aborted => "a service failed, taking the group down",
        }
    }
}

/// First reason to arrive wins: a service dying during a Ctrl-C should not
/// relabel the run as an abort, nor the other way round.
#[derive(Clone)]
struct Shutdown {
    tx: Arc<watch::Sender<Option<Reason>>>,
}

impl Shutdown {
    fn new() -> Self {
        let (tx, _) = watch::channel(None);
        Self { tx: Arc::new(tx) }
    }

    fn request(&self, reason: Reason) {
        self.tx.send_if_modified(|slot| {
            if slot.is_some() {
                return false;
            }
            *slot = Some(reason);
            true
        });
    }

    fn reason(&self) -> Option<Reason> {
        *self.tx.borrow()
    }

    fn subscribe(&self) -> watch::Receiver<Option<Reason>> {
        self.tx.subscribe()
    }
}

/// Resolves the first time a reason is published. Parks forever if the sender
/// is gone without one, so the caller's other `select!` arms still decide.
async fn wait_for_shutdown(rx: &mut watch::Receiver<Option<Reason>>) -> Reason {
    loop {
        let current = *rx.borrow_and_update();
        if let Some(reason) = current {
            return reason;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// What a node publishes to its dependents. Anything but `Ready` means they
/// must not start.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Readiness {
    Ready,
    Unavailable,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    // Before anything that can take a measurable moment -- reading the spec,
    // building the HTTP client -- because until this is installed the default
    // action for SIGINT is to kill the runner outright, and a Ctrl-C in that
    // window leaves every child it had already spawned running.
    let sigint = signal(SignalKind::interrupt()).context("installing SIGINT handler")?;

    let args = Args::parse();
    let text = std::fs::read_to_string(&args.spec)
        .with_context(|| format!("reading spec: {}", args.spec.display()))?;
    let mut spec: Spec = serde_json::from_str(&text).context("parsing spec JSON")?;

    if !args.only.is_empty() {
        filter_only(&mut spec, &args.only)?;
    }

    validate(&spec)?;

    let mode = resolve_mode(args.output, &spec)?;
    let http = reqwest::Client::builder()
        .build()
        .context("building the HTTP client readiness probes use")?;
    let started = Instant::now();

    let shutdown = Shutdown::new();
    spawn_cancel_listener(shutdown.clone(), sigint);

    let records = run(spec, mode, started, &shutdown, http).await;

    if matches!(mode, OutputMode::Json) {
        emit_summary(&records, started);
    } else {
        print_summary(&records, started);
    }

    // A cancellation always exits 130, even if every node managed to finish
    // (succeed, fail, or skip) before being killed. Callers distinguish
    // "cancelled by the operator" from "ran to completion" via this code.
    if shutdown.reason() == Some(Reason::Cancelled) {
        std::process::exit(130);
    }
    std::process::exit(exit_code(&records));
}

/// Background task: first Ctrl-C broadcasts cancellation; second hard-exits.
fn spawn_cancel_listener(shutdown: Shutdown, mut sigint: Signal) {
    tokio::spawn(async move {
        if sigint.recv().await.is_none() {
            return;
        }
        eprintln!(
            "dag-runner: SIGINT received, cancelling running nodes (Ctrl-C again to hard-exit)"
        );
        shutdown.request(Reason::Cancelled);
        if sigint.recv().await.is_some() {
            eprintln!("dag-runner: second SIGINT, hard-exiting");
            std::process::exit(130);
        }
    });
}

/// Pick the renderer, and refuse the two combinations that would corrupt it.
/// A node with `"stdio": "inherit"` writes straight to this process's stdout
/// and stderr, which walks over indicatif's spinners and over the NDJSON
/// stream. `auto` picks the mode that works, since choosing is its job; an
/// explicit `--output` gets an error naming the nodes rather than a run whose
/// output cannot be trusted.
fn resolve_mode(requested: OutputMode, spec: &Spec) -> Result<OutputMode> {
    let mut inheriting: Vec<&str> = spec
        .nodes
        .iter()
        .filter(|(_, node)| matches!(node.stdio, StdioMode::Inherit))
        .map(|(name, _)| name.as_str())
        .collect();
    inheriting.sort_unstable();

    match requested {
        OutputMode::Auto => Ok(
            if inheriting.is_empty() && std::io::stdout().is_terminal() {
                OutputMode::Tui
            } else {
                OutputMode::Plain
            },
        ),
        OutputMode::Tui if !inheriting.is_empty() => bail!(
            "--output tui cannot be used with \"stdio\": \"inherit\" ({}): an inherited child \
             writes over the spinners. Use --output plain, or set those nodes to \"prefixed\".",
            inheriting.join(", ")
        ),
        OutputMode::Json if !inheriting.is_empty() => bail!(
            "--output json cannot be used with \"stdio\": \"inherit\" ({}): an inherited child \
             writes into the NDJSON stream on stdout. Set those nodes to \"prefixed\", whose echo \
             goes to stderr.",
            inheriting.join(", ")
        ),
        mode => Ok(mode),
    }
}

fn validate(spec: &Spec) -> Result<()> {
    for (name, node) in &spec.nodes {
        validate_node(name, node)?;
        for dep in &node.depends_on {
            if !spec.nodes.contains_key(dep) {
                bail!("node {name} depends on unknown node {dep}");
            }
        }
    }
    detect_cycle(&spec.nodes)?;
    Ok(())
}

/// Every combination rejected here is one whose failure mode is silence: a
/// probe that is never read, a service nothing waits for, a descriptor that
/// lands on the child's own stdout.
fn validate_node(name: &str, node: &NodeSpec) -> Result<()> {
    if node.command.is_empty() {
        bail!("node {name} has empty command");
    }

    match (node.kind, node.ready_when.as_ref()) {
        (Kind::Task, Some(_)) => bail!(
            "node {name} is a task but sets ready_when; a task is ready when it exits zero. \
             Set \"kind\": \"service\" if it is meant to stay up."
        ),
        (Kind::Service, None) => bail!(
            "node {name} is a service but sets no ready_when; without one a dependent would \
             start against a process that has not finished coming up. Add a tcp, http, or \
             log_line probe."
        ),
        _ => {}
    }

    if node.is_service() && node.timeout_secs.is_some() {
        bail!(
            "node {name} is a service but sets timeout_secs; a service is expected to stay up \
             for the whole run. Use ready_timeout_secs to bound how long it may take to come up."
        );
    }

    if matches!(node.stdio, StdioMode::Inherit)
        && matches!(node.ready_when, Some(ReadyWhen::LogLine(_)))
    {
        bail!(
            "node {name} asks for log_line readiness with \"stdio\": \"inherit\"; an inherited \
             stream goes straight to the terminal and the runner never sees a line to match. Use \
             the default \"capture\", or \"prefixed\"."
        );
    }

    if let Some(fd) = node.lifeline_fd
        && fd < 3
    {
        bail!(
            "node {name} sets lifeline_fd {fd}; 0, 1 and 2 are the child's own stdio and a \
             negative number is not a descriptor. Pick 3 or above."
        );
    }

    Ok(())
}

/// Trim `spec.nodes` to the names in `only`. Unknown names and edges left
/// dangling by the cut are rejected here, so the remaining graph keeps the
/// "every kept node has every dep it needs" invariant of an unfiltered run.
fn filter_only(spec: &mut Spec, only: &[String]) -> Result<()> {
    let keep: HashSet<&str> = only.iter().map(String::as_str).collect();

    let mut missing: Vec<&str> = keep
        .iter()
        .copied()
        .filter(|name| !spec.nodes.contains_key(*name))
        .collect();
    if !missing.is_empty() {
        missing.sort_unstable();
        bail!("--only names not found in spec: {}", missing.join(", "));
    }

    spec.nodes.retain(|name, _| keep.contains(name.as_str()));

    let mut dangling: Vec<String> = spec
        .nodes
        .iter()
        .flat_map(|(name, node)| {
            node.depends_on
                .iter()
                .filter(|dep| !keep.contains(dep.as_str()))
                .map(move |dep| format!("{name} -> {dep}"))
        })
        .collect();
    if !dangling.is_empty() {
        dangling.sort();
        bail!(
            "--only would drop dependencies of kept nodes: {}; add the missing names to --only",
            dangling.join(", ")
        );
    }

    Ok(())
}

fn detect_cycle(nodes: &HashMap<String, NodeSpec>) -> Result<()> {
    let mut color: HashMap<&str, CycleColor> = HashMap::new();

    let mut names: Vec<&str> = nodes.keys().map(String::as_str).collect();
    names.sort_unstable();
    for name in names {
        let mut stack = Vec::new();
        visit_cycle(name, nodes, &mut color, &mut stack)?;
    }
    Ok(())
}

fn visit_cycle<'a>(
    name: &'a str,
    nodes: &'a HashMap<String, NodeSpec>,
    color: &mut HashMap<&'a str, CycleColor>,
    stack: &mut Vec<&'a str>,
) -> Result<()> {
    match color.get(name) {
        Some(CycleColor::Gray) => {
            stack.push(name);
            bail!("cycle detected: {}", stack.join(" -> "));
        }
        Some(CycleColor::Black) => return Ok(()),
        None => {}
    }
    color.insert(name, CycleColor::Gray);
    stack.push(name);
    for dep in &nodes[name].depends_on {
        visit_cycle(dep, nodes, color, stack)?;
    }
    stack.pop();
    color.insert(name, CycleColor::Black);
    Ok(())
}

fn topological_order(nodes: &HashMap<String, NodeSpec>) -> Vec<String> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::with_capacity(nodes.len());

    // Deterministic walk so the spawn order matches the spec's lexicographic
    // node order rather than a HashMap iteration accident; this keeps log
    // output stable across runs.
    let mut names: Vec<&String> = nodes.keys().collect();
    names.sort();
    for name in names {
        visit_topo(name, nodes, &mut visited, &mut order);
    }
    order
}

fn visit_topo(
    name: &str,
    nodes: &HashMap<String, NodeSpec>,
    visited: &mut HashSet<String>,
    order: &mut Vec<String>,
) {
    if !visited.insert(name.to_string()) {
        return;
    }
    for dep in &nodes[name].depends_on {
        visit_topo(dep, nodes, visited, order);
    }
    order.push(name.to_string());
}

/// Everything one node's future needs that is not the graph itself.
struct NodeContext {
    name: String,
    node: NodeSpec,
    mode: OutputMode,
    started: Instant,
    pb: Option<ProgressBar>,
    multi: Option<MultiProgress>,
    shutdown: Shutdown,
    http: reqwest::Client,
}

async fn run(
    spec: Spec,
    mode: OutputMode,
    started: Instant,
    shutdown: &Shutdown,
    http: reqwest::Client,
) -> BTreeMap<String, NodeRecord> {
    let multi = matches!(mode, OutputMode::Tui).then(MultiProgress::new);
    let records: Arc<Mutex<BTreeMap<String, NodeRecord>>> = Arc::new(Mutex::new(BTreeMap::new()));

    let order = topological_order(&spec.nodes);
    // Services never settle on their own, so the run is over when the last
    // task is. A spec with no tasks in it never trips this, which is how a
    // pure supervision group stays up until the operator stops it.
    let remaining_tasks = Arc::new(AtomicUsize::new(
        spec.nodes.values().filter(|node| !node.is_service()).count(),
    ));

    let mut ready: HashMap<String, watch::Receiver<Option<Readiness>>> =
        HashMap::with_capacity(order.len());
    let mut handles = Vec::with_capacity(order.len());

    for name in &order {
        let node = spec.nodes[name].clone();
        let mut deps: Vec<watch::Receiver<Option<Readiness>>> = node
            .depends_on
            .iter()
            .map(|dep| ready[dep].clone())
            .collect();
        let (ready_tx, ready_rx) = watch::channel(None);
        ready.insert(name.clone(), ready_rx);

        let is_service = node.is_service();
        let ctx = NodeContext {
            name: name.clone(),
            pb: multi.as_ref().map(|m| make_spinner(m, name)),
            multi: multi.clone(),
            node,
            mode,
            started,
            shutdown: shutdown.clone(),
            http: http.clone(),
        };

        let records_for_task = Arc::clone(&records);
        let remaining_for_task = Arc::clone(&remaining_tasks);
        let shutdown_for_task = shutdown.clone();
        let key = name.clone();

        handles.push(tokio::spawn(async move {
            let record = run_node(&ctx, &mut deps, &ready_tx).await;
            records_for_task.lock().await.insert(key, record);
            if !is_service && remaining_for_task.fetch_sub(1, Ordering::SeqCst) == 1 {
                shutdown_for_task.request(Reason::Drained);
            }
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    if let Some(multi) = multi {
        let _ = multi.clear();
    }

    std::mem::take(&mut *records.lock().await)
}

/// Blocks until every dependency has published a verdict. `false` means at
/// least one of them will never be ready, so the caller must not run.
async fn await_deps(deps: &mut [watch::Receiver<Option<Readiness>>]) -> bool {
    for rx in deps {
        loop {
            let current = *rx.borrow_and_update();
            match current {
                Some(Readiness::Ready) => break,
                Some(Readiness::Unavailable) => return false,
                // A node's task dropping its sender without a verdict is a
                // missing answer, not a good one. Reading it as unavailable is
                // what keeps a panicking node from hanging the whole graph.
                None if rx.changed().await.is_err() => return false,
                None => {}
            }
        }
    }
    true
}

async fn run_node(
    ctx: &NodeContext,
    deps: &mut [watch::Receiver<Option<Readiness>>],
    ready_tx: &watch::Sender<Option<Readiness>>,
) -> NodeRecord {
    if !await_deps(deps).await {
        let _ = ready_tx.send(Some(Readiness::Unavailable));
        return never_ran(ctx, None);
    }

    if let Some(reason) = ctx.shutdown.reason() {
        let _ = ready_tx.send(Some(Readiness::Unavailable));
        return never_ran(ctx, Some(reason));
    }

    report_started(&ctx.name, ctx.started, ctx.mode, ctx.pb.as_ref());
    let node_started = Instant::now();
    let output = execute(ctx, ready_tx).await;
    let duration = node_started.elapsed();

    // A service publishes its own verdict, at the moment its probe passes
    // rather than when it stops.
    if !ctx.node.is_service() {
        let verdict = if matches!(output.outcome, Outcome::Succeeded) {
            Readiness::Ready
        } else {
            Readiness::Unavailable
        };
        let _ = ready_tx.send(Some(verdict));
    }

    report_finished(
        &ctx.name,
        &output.outcome,
        duration,
        ctx.started,
        ctx.mode,
        ctx.pb.as_ref(),
    );

    NodeRecord {
        outcome: output.outcome,
        duration,
        stdout: output.stdout,
        stderr: output.stderr,
    }
}

/// A node that never got as far as spawning: a dependency will never be
/// ready, or the group was already on its way down.
fn never_ran(ctx: &NodeContext, reason: Option<Reason>) -> NodeRecord {
    let duration = Duration::ZERO;
    report_finished(
        &ctx.name,
        &Outcome::Skipped,
        duration,
        ctx.started,
        ctx.mode,
        ctx.pb.as_ref(),
    );
    let mut stderr = OutputTail::new();
    if let Some(reason) = reason {
        stderr.push(format!("dag-runner: never started: {}\n", reason.describe()));
    }
    NodeRecord {
        outcome: Outcome::Skipped,
        duration,
        stdout: OutputTail::new(),
        stderr,
    }
}

fn make_spinner(multi: &MultiProgress, name: &str) -> ProgressBar {
    let pb = multi.add(ProgressBar::new_spinner());
    pb.set_style(progress_style::spinner());
    pb.set_prefix(name.to_string());
    pb.set_message("pending");
    pb
}

/// The result of running one node's command: its outcome and retained output.
struct CommandOutput {
    outcome: Outcome,
    stdout: OutputTail,
    stderr: OutputTail,
}

struct CapturedExit {
    status: io::Result<ExitStatus>,
    stdout: OutputTail,
    stderr: OutputTail,
}

struct CapturedStreams {
    stdout: OutputTail,
    stderr: OutputTail,
}

struct PipeCapture {
    stdout_task: Option<tokio::task::JoinHandle<OutputTail>>,
    stderr_task: Option<tokio::task::JoinHandle<OutputTail>>,
    stdout: Option<OutputTail>,
    stderr: Option<OutputTail>,
}

enum PipeTaskCompletion {
    Stdout(Result<OutputTail, tokio::task::JoinError>),
    Stderr(Result<OutputTail, tokio::task::JoinError>),
}

impl PipeCapture {
    const fn new(
        stdout_task: tokio::task::JoinHandle<OutputTail>,
        stderr_task: tokio::task::JoinHandle<OutputTail>,
    ) -> Self {
        Self {
            stdout_task: Some(stdout_task),
            stderr_task: Some(stderr_task),
            stdout: None,
            stderr: None,
        }
    }

    /// A node with `"stdio": "inherit"` has no pipes to drain, so every
    /// operation here is already done. `finish` returning immediately is what
    /// makes an inherited child's exit visible the instant it happens rather
    /// than when a stream closes.
    const fn none() -> Self {
        Self {
            stdout_task: None,
            stderr_task: None,
            stdout: None,
            stderr: None,
        }
    }

    async fn finish(&mut self) {
        loop {
            let completion = match (&mut self.stdout_task, &mut self.stderr_task) {
                (Some(stdout_task), Some(stderr_task)) => tokio::select! {
                    result = stdout_task => PipeTaskCompletion::Stdout(result),
                    result = stderr_task => PipeTaskCompletion::Stderr(result),
                },
                (Some(stdout_task), None) => {
                    PipeTaskCompletion::Stdout(stdout_task.await)
                }
                (None, Some(stderr_task)) => {
                    PipeTaskCompletion::Stderr(stderr_task.await)
                }
                (None, None) => return,
            };
            match completion {
                PipeTaskCompletion::Stdout(result) => {
                    self.stdout = Some(result.unwrap_or_default());
                    self.stdout_task = None;
                }
                PipeTaskCompletion::Stderr(result) => {
                    self.stderr = Some(result.unwrap_or_default());
                    self.stderr_task = None;
                }
            }
        }
    }

    fn abort(&self) {
        if let Some(task) = &self.stdout_task {
            task.abort();
        }
        if let Some(task) = &self.stderr_task {
            task.abort();
        }
    }

    fn take(&mut self) -> CapturedStreams {
        CapturedStreams {
            stdout: self.stdout.take().unwrap_or_default(),
            stderr: self.stderr.take().unwrap_or_default(),
        }
    }
}

#[derive(Clone, Copy)]
struct ProcessGroupId(libc::pid_t);

impl ProcessGroupId {
    fn from_child(child: &Child) -> Self {
        let pid = child.id().expect("spawned child has a PID");
        Self(pid.cast_signed())
    }

    fn signal(self, signal: libc::c_int) -> io::Result<()> {
        // SAFETY: the command was spawned with this PID as its process group,
        // and `signal` is one of the libc signal constants used below.
        if unsafe { libc::killpg(self.0, signal) } == 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if Self::means_already_gone(&error) {
            Ok(())
        } else {
            Err(error)
        }
    }

    /// Whether the kernel is saying "there is nothing left in that group".
    ///
    /// ESRCH is the portable spelling. Darwin also answers EPERM when the only
    /// member left is a zombie the caller has not reaped yet -- which is
    /// precisely the state `terminate_process_group` engineers on purpose, by
    /// holding the leader unreaped until after the KILL so its group ID cannot
    /// be recycled during the grace. Reading that as a real failure made every
    /// cancelled or timed-out node on macOS report a teardown error that never
    /// happened, and skipped the `disarm` that follows, so `Drop` then printed
    /// a second one. Reproduced standalone: fork a child into its own group,
    /// SIGTERM it, leave it unreaped, and `killpg` gives EPERM; reap it and the
    /// same call gives ESRCH.
    ///
    /// Kept to Darwin so a genuine permission failure on Linux -- a child that
    /// changed credentials out from under us -- still surfaces.
    fn means_already_gone(error: &io::Error) -> bool {
        if error.raw_os_error() == Some(libc::ESRCH) {
            return true;
        }
        cfg!(target_os = "macos") && error.raw_os_error() == Some(libc::EPERM)
    }
}

struct OwnedProcessGroup {
    id: ProcessGroupId,
    armed: bool,
}

impl OwnedProcessGroup {
    fn new(child: &Child) -> Self {
        Self {
            id: ProcessGroupId::from_child(child),
            armed: true,
        }
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }

    const fn is_armed(&self) -> bool {
        self.armed
    }
}

impl Drop for OwnedProcessGroup {
    fn drop(&mut self) {
        if self.armed && let Err(error) = self.id.signal(libc::SIGKILL) {
            eprintln!(
                "dag-runner: failed to kill process group {} during cleanup: {error}",
                self.id.0
            );
        }
    }
}

fn build_command(node: &NodeSpec) -> Command {
    let mut cmd = Command::new(&node.command[0]);
    cmd.args(&node.command[1..])
        .envs(&node.env)
        // The group ID becomes the child's PID. Descendants inherit it unless
        // they deliberately leave, so one owned identity covers the node tree.
        .process_group(0);
    if matches!(node.stdio, StdioMode::Inherit) {
        cmd.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    cmd
}

async fn execute(ctx: &NodeContext, ready_tx: &watch::Sender<Option<Readiness>>) -> CommandOutput {
    let mut cmd = build_command(&ctx.node);

    let lifeline = match ctx.node.lifeline_fd {
        Some(fd) => match attach_lifeline(&mut cmd, fd) {
            Ok(pipe) => Some(pipe),
            Err(error) => return spawn_failed(ctx, ready_tx, &error),
        },
        None => None,
    };

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) => return spawn_failed(ctx, ready_tx, &error),
    };
    // The child holds its own copy of the read end now, so ours would only be
    // a descriptor leak. The write end is the half whose closing it sees.
    let _lifeline = lifeline.map(|pipe| pipe.write);
    let mut group = OwnedProcessGroup::new(&child);

    let (hit_tx, mut hit_rx) = watch::channel(false);
    let mut capture = if matches!(ctx.node.stdio, StdioMode::Inherit) {
        PipeCapture::none()
    } else {
        let stdout_pipe = child.stdout.take().expect("stdout piped");
        let stderr_pipe = child.stderr.take().expect("stderr piped");
        let hit = Arc::new(hit_tx);
        PipeCapture::new(
            tokio::spawn(tee_lines(
                stdout_pipe,
                make_sink(ctx, LogStream::Stdout, Arc::clone(&hit)),
            )),
            tokio::spawn(tee_lines(
                stderr_pipe,
                make_sink(ctx, LogStream::Stderr, hit),
            )),
        )
    };

    if ctx.node.is_service() {
        run_service(ctx, &mut child, &mut group, &mut capture, ready_tx, &mut hit_rx).await
    } else {
        run_task(ctx, &mut child, &mut group, &mut capture).await
    }
}

fn spawn_failed(
    ctx: &NodeContext,
    ready_tx: &watch::Sender<Option<Readiness>>,
    error: &io::Error,
) -> CommandOutput {
    if ctx.node.is_service() {
        let _ = ready_tx.send(Some(Readiness::Unavailable));
        ctx.shutdown.request(Reason::Aborted);
    }
    let mut stderr = OutputTail::new();
    stderr.push(format!("failed to spawn: {error}\n"));
    CommandOutput {
        outcome: Outcome::Failed(127),
        stdout: OutputTail::new(),
        stderr,
    }
}

/// Drain what the readers have and hand it back. A still-armed group means
/// termination did not reap cleanly, so a descendant may hold a stream open
/// indefinitely; abort the readers rather than wait on them.
async fn collect(capture: &mut PipeCapture, group: &OwnedProcessGroup) -> CapturedStreams {
    if group.is_armed() {
        capture.abort();
    }
    capture.finish().await;
    capture.take()
}

/// Terminate the group, drain, and label why. Shared by every path that stops
/// a child the runner is still holding.
async fn stop(
    child: &mut Child,
    group: &mut OwnedProcessGroup,
    capture: &mut PipeCapture,
    note: String,
) -> CapturedStreams {
    let result = terminate_process_group(child, group).await;
    let CapturedStreams { stdout, mut stderr } = collect(capture, group).await;
    record_termination_failure(&mut stderr, result);
    stderr.push(note);
    CapturedStreams { stdout, stderr }
}

async fn run_task(
    ctx: &NodeContext,
    child: &mut Child,
    group: &mut OwnedProcessGroup,
    capture: &mut PipeCapture,
) -> CommandOutput {
    let mut rx = ctx.shutdown.subscribe();
    let completion = tokio::select! {
        biased;
        reason = wait_for_shutdown(&mut rx) => Completion::ShutDown(reason),
        secs = maybe_timeout(ctx.node.timeout_secs) => Completion::TimedOut { secs },
        exit = capture_exit(child, capture) => Completion::Exited(exit),
    };

    match completion {
        Completion::Exited(mut exit) => {
            let outcome = match exit.status {
                Ok(status) => {
                    group.disarm();
                    if status.success() {
                        Outcome::Succeeded
                    } else {
                        Outcome::Failed(status.code().unwrap_or(1))
                    }
                }
                Err(error) => {
                    exit.stderr.push(format!("wait failed: {error}\n"));
                    record_termination_failure(
                        &mut exit.stderr,
                        terminate_process_group(child, group).await,
                    );
                    Outcome::Failed(1)
                }
            };
            CommandOutput {
                outcome,
                stdout: exit.stdout,
                stderr: exit.stderr,
            }
        }
        Completion::TimedOut { secs } => {
            let streams = stop(
                child,
                group,
                capture,
                format!("dag-runner: node timed out after {secs}s\n"),
            )
            .await;
            CommandOutput {
                outcome: Outcome::Failed(124),
                stdout: streams.stdout,
                stderr: streams.stderr,
            }
        }
        Completion::ShutDown(reason) => {
            let streams = stop(
                child,
                group,
                capture,
                format!("dag-runner: terminated: {}\n", reason.describe()),
            )
            .await;
            CommandOutput {
                outcome: Outcome::Failed(reason.terminated_exit_code()),
                stdout: streams.stdout,
                stderr: streams.stderr,
            }
        }
    }
}

async fn run_service(
    ctx: &NodeContext,
    child: &mut Child,
    group: &mut OwnedProcessGroup,
    capture: &mut PipeCapture,
    ready_tx: &watch::Sender<Option<Readiness>>,
    hit_rx: &mut watch::Receiver<bool>,
) -> CommandOutput {
    let mut rx = ctx.shutdown.subscribe();
    let ready_when = ctx
        .node
        .ready_when
        .as_ref()
        .expect("validation requires ready_when on a service");

    let startup = tokio::select! {
        biased;
        reason = wait_for_shutdown(&mut rx) => Startup::ShutDown(reason),
        () = tokio::time::sleep(Duration::from_secs(ctx.node.ready_timeout_secs)) => Startup::TimedOut,
        exit = capture_exit(child, capture) => Startup::Exited(exit),
        () = wait_until_ready(ready_when, hit_rx, &ctx.http) => Startup::Ready,
    };

    if !matches!(startup, Startup::Ready) {
        let _ = ready_tx.send(Some(Readiness::Unavailable));
        return never_came_up(ctx, child, group, capture, startup).await;
    }

    report_ready(&ctx.name, ctx.started, ctx.mode, ctx.pb.as_ref());
    let _ = ready_tx.send(Some(Readiness::Ready));

    tokio::select! {
        biased;
        _ = wait_for_shutdown(&mut rx) => {
            let streams = stop(child, group, capture, String::new()).await;
            CommandOutput {
                outcome: Outcome::Stopped,
                stdout: streams.stdout,
                stderr: streams.stderr,
            }
        }
        exit = capture_exit(child, capture) => {
            // A service that exits while the run is still going has taken the
            // thing its dependents are talking to with it, so nothing after
            // this point can produce a result anyone should believe.
            ctx.shutdown.request(Reason::Aborted);
            service_exit(group, exit, "while the run was still going")
        }
    }
}

async fn never_came_up(
    ctx: &NodeContext,
    child: &mut Child,
    group: &mut OwnedProcessGroup,
    capture: &mut PipeCapture,
    startup: Startup,
) -> CommandOutput {
    match startup {
        // A service the runner stopped before it came up did not fail. The
        // node that caused the shutdown did, and that is where the exit code
        // comes from.
        Startup::ShutDown(reason) => {
            let streams = stop(
                child,
                group,
                capture,
                format!(
                    "dag-runner: stopped before it became ready: {}\n",
                    reason.describe()
                ),
            )
            .await;
            CommandOutput {
                outcome: Outcome::Stopped,
                stdout: streams.stdout,
                stderr: streams.stderr,
            }
        }
        Startup::TimedOut => {
            ctx.shutdown.request(Reason::Aborted);
            let streams = stop(
                child,
                group,
                capture,
                format!(
                    "dag-runner: service was not ready after {}s\n",
                    ctx.node.ready_timeout_secs
                ),
            )
            .await;
            CommandOutput {
                outcome: Outcome::Failed(124),
                stdout: streams.stdout,
                stderr: streams.stderr,
            }
        }
        Startup::Exited(exit) => {
            ctx.shutdown.request(Reason::Aborted);
            service_exit(group, exit, "before it became ready")
        }
        Startup::Ready => unreachable!("the caller handles the ready case"),
    }
}

/// A service's own process went away. The leader is already reaped by the time
/// this runs, so the group is disarmed rather than signalled: `killpg` on a
/// reaped leader's ID is the reuse hazard `terminate_process_group` exists to
/// avoid.
fn service_exit(group: &mut OwnedProcessGroup, exit: CapturedExit, when: &str) -> CommandOutput {
    let CapturedExit {
        status,
        stdout,
        mut stderr,
    } = exit;
    let code = match &status {
        Ok(status) => {
            group.disarm();
            status.code().unwrap_or(1)
        }
        Err(_) => 1,
    };
    stderr.push(format!(
        "dag-runner: service exited (status {code}) {when}\n"
    ));
    CommandOutput {
        outcome: Outcome::Failed(code),
        stdout,
        stderr,
    }
}

/// Both ends of a lifeline pipe. The read end has to outlive the spawn so the
/// forked child can `dup2` it; the write end outlives the child.
struct Lifeline {
    read: OwnedFd,
    write: OwnedFd,
}

fn attach_lifeline(cmd: &mut Command, target_fd: i32) -> io::Result<Lifeline> {
    let lifeline = new_pipe()?;
    let raw_read = lifeline.read.as_raw_fd();

    // SAFETY: the closure runs between fork and exec, where only
    // async-signal-safe calls are legal. `dup2` and `fcntl` both are, and
    // neither allocates, takes a lock, nor touches anything but the descriptor
    // table this child now owns privately.
    unsafe {
        cmd.as_std_mut().pre_exec(move || {
            if raw_read == target_fd {
                // `pipe(2)` hands out the lowest free descriptors, so asking
                // for fd 3 -- the usual choice -- often lands on the read end
                // itself. `dup2(n, n)` is a documented no-op that leaves
                // CLOEXEC set, which would close the lifeline at exec and make
                // the whole guard quietly do nothing.
                if libc::fcntl(raw_read, libc::F_SETFD, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
            } else if libc::dup2(raw_read, target_fd) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }

    Ok(lifeline)
}

fn new_pipe() -> io::Result<Lifeline> {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: `pipe(2)` writes exactly two ints into the array it is handed,
    // and `fds` is exactly that.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    for fd in fds {
        // macOS has no `pipe2`, so CLOEXEC goes on afterwards. The window
        // between the two only leaks if another thread forks inside it, and
        // the runtime here is `current_thread`: every spawn happens on this
        // one thread, in order.
        //
        // SAFETY: `fd` was just returned by `pipe(2)` and nothing has closed
        // it.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    // SAFETY: both descriptors come from the `pipe(2)` above and nothing else
    // owns them, which is what `OwnedFd` requires.
    unsafe {
        Ok(Lifeline {
            read: OwnedFd::from_raw_fd(fds[0]),
            write: OwnedFd::from_raw_fd(fds[1]),
        })
    }
}

/// Gap between readiness attempts. Matches the 100ms poll the shell launchers
/// this replaces used: short enough that a local server's startup is not
/// dominated by the wait, long enough not to spin.
const PROBE_INTERVAL: Duration = Duration::from_millis(100);

/// How long one attempt may hang before it counts as a miss. Connecting to a
/// port nothing listens on fails fast, but a port that accepts and then stalls
/// would otherwise pin the probe until the readiness deadline.
const PROBE_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);

/// Blocks until the condition holds. Deliberately has no deadline of its own:
/// the caller races it against the child exiting, the readiness timeout, and
/// group shutdown, so exactly one place decides what running out of time
/// means.
async fn wait_until_ready(
    ready_when: &ReadyWhen,
    hit_rx: &mut watch::Receiver<bool>,
    http: &reqwest::Client,
) {
    match ready_when {
        ReadyWhen::Tcp(probe) => loop {
            if let Ok(Ok(stream)) =
                tokio::time::timeout(PROBE_ATTEMPT_TIMEOUT, TcpStream::connect(&probe.address))
                    .await
            {
                drop(stream);
                return;
            }
            tokio::time::sleep(PROBE_INTERVAL).await;
        },
        ReadyWhen::Http(probe) => loop {
            if http_attempt(probe, http).await {
                return;
            }
            tokio::time::sleep(PROBE_INTERVAL).await;
        },
        // The match runs inside the stream reader, so this only waits on the
        // flag it sets.
        ReadyWhen::LogLine(_) => loop {
            if *hit_rx.borrow_and_update() {
                return;
            }
            if hit_rx.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        },
    }
}

async fn http_attempt(probe: &HttpReady, client: &reqwest::Client) -> bool {
    let Ok(response) = client
        .get(&probe.url)
        .timeout(PROBE_ATTEMPT_TIMEOUT)
        .send()
        .await
    else {
        return false;
    };
    if response.status().as_u16() != probe.status {
        return false;
    }
    let Some(needle) = probe.body_contains.clone() else {
        return true;
    };
    response
        .text()
        .await
        .is_ok_and(|body| body.contains(&needle))
}

fn record_termination_failure(stderr: &mut OutputTail, result: io::Result<()>) {
    if let Err(error) = result {
        stderr.push(format!(
            "dag-runner: failed to terminate node process group: {error}\n"
        ));
    }
}

enum Completion {
    Exited(CapturedExit),
    TimedOut { secs: u64 },
    ShutDown(Reason),
}

/// What ended a service's start-up race.
enum Startup {
    Ready,
    Exited(CapturedExit),
    TimedOut,
    ShutDown(Reason),
}

/// Drain both captured streams before reaping the group leader. If a
/// descendant keeps either stream open, timeout and cancellation still own a
/// live leader PID, so the process group identity cannot be reused.
async fn capture_exit(child: &mut Child, capture: &mut PipeCapture) -> CapturedExit {
    capture.finish().await;
    let status = child.wait().await;
    let CapturedStreams { stdout, stderr } = capture.take();
    CapturedExit {
        status,
        stdout,
        stderr,
    }
}

/// Resolves after `secs` seconds when set, otherwise blocks forever. Used as
/// the timeout arm of a `tokio::select!`: pairing it with `child.wait()`
/// lets the wait win when no timeout was requested.
async fn maybe_timeout(secs: Option<u64>) -> u64 {
    match secs {
        Some(s) => {
            tokio::time::sleep(Duration::from_secs(s)).await;
            s
        }
        None => std::future::pending::<u64>().await,
    }
}

/// Give the whole owned process group a brief TERM grace period, then KILL it
/// and reap the direct child. The group leader stays unreaped until after KILL,
/// which prevents its numeric group ID from being reused during the grace.
async fn terminate_process_group(
    child: &mut Child,
    group: &mut OwnedProcessGroup,
) -> io::Result<()> {
    let term_result = group.id.signal(libc::SIGTERM);
    tokio::time::sleep(Duration::from_millis(500)).await;
    group.id.signal(libc::SIGKILL)?;
    group.disarm();
    let wait_result = child.wait().await.map(|_| ());

    term_result?;
    wait_result
}

/// Everything one stream reader does with a line besides retain it: drive the
/// spinner, echo it under `"stdio": "prefixed"`, and fire a `log_line`
/// readiness probe.
struct Sink {
    node: String,
    which: LogStream,
    echo: bool,
    pb: Option<ProgressBar>,
    multi: Option<MultiProgress>,
    matcher: Option<Matcher>,
}

struct Matcher {
    pattern: String,
    stream: LogStream,
    hit: Arc<watch::Sender<bool>>,
}

fn make_sink(ctx: &NodeContext, which: LogStream, hit: Arc<watch::Sender<bool>>) -> Sink {
    let matcher = match ctx.node.ready_when.as_ref() {
        Some(ReadyWhen::LogLine(probe)) => Some(Matcher {
            pattern: probe.pattern.clone(),
            stream: probe.stream,
            hit,
        }),
        _ => None,
    };
    Sink {
        node: ctx.name.clone(),
        which,
        echo: matches!(ctx.node.stdio, StdioMode::Prefixed),
        pb: ctx.pb.clone(),
        multi: ctx.multi.clone(),
        matcher,
    }
}

impl Sink {
    fn accept(&self, line: &str) {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if let Some(matcher) = &self.matcher
            && (matches!(matcher.stream, LogStream::Either) || matcher.stream == self.which)
            && trimmed.contains(&matcher.pattern)
        {
            let _ = matcher.hit.send(true);
        }
        if trimmed.is_empty() {
            return;
        }
        if self.echo {
            let text = format!("{} | {trimmed}", self.node);
            // Above the spinners when there are spinners, and always on
            // stderr, so `--output json`'s stdout stream stays parseable.
            match self.multi.as_ref() {
                Some(multi) => {
                    let _ = multi.println(&text);
                }
                None => eprintln!("{text}"),
            }
        }
        if let Some(pb) = &self.pb {
            pb.set_message(truncate_for_spinner(trimmed));
        }
    }
}

/// Read `stream` line-by-line, handing each line to the sink and keeping a
/// bounded tail of it. The spinner shows the newest line, so a long-running
/// node looks alive instead of just ticking elapsed.
async fn tee_lines(stream: impl AsyncRead + Unpin, sink: Sink) -> OutputTail {
    let mut reader = BufReader::new(stream);
    let mut tail = OutputTail::new();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                sink.accept(&line);
                tail.push(line.clone());
            }
        }
    }
    tail
}

/// Clip a line to a single-row display width. Char-aware so multibyte
/// terminal output doesn't get sliced mid-codepoint.
fn truncate_for_spinner(line: &str) -> String {
    const MAX: usize = 80;
    let count = line.chars().count();
    if count <= MAX {
        line.to_string()
    } else {
        let mut out: String = line.chars().take(MAX - 1).collect();
        out.push('…');
        out
    }
}

fn report_started(name: &str, started: Instant, mode: OutputMode, pb: Option<&ProgressBar>) {
    match mode {
        OutputMode::Tui => {
            if let Some(pb) = pb {
                pb.set_message("running");
                pb.enable_steady_tick(Duration::from_millis(100));
            }
        }
        OutputMode::Plain => {
            println!(
                "[{:>6.1}s] {} started",
                started.elapsed().as_secs_f64(),
                name
            );
        }
        OutputMode::Json => {
            emit(&Event::NodeStarted {
                node: name,
                ts_ms: started.elapsed().as_millis(),
            });
        }
        OutputMode::Auto => unreachable!("auto resolved earlier"),
    }
}

/// A service passed its probe. Its dependents start after this, so the line
/// is the boundary between "coming up" and "in use".
fn report_ready(name: &str, started: Instant, mode: OutputMode, pb: Option<&ProgressBar>) {
    match mode {
        OutputMode::Tui => {
            if let Some(pb) = pb {
                pb.set_message("ready");
            }
        }
        OutputMode::Plain => {
            println!("[{:>6.1}s] {} ready", started.elapsed().as_secs_f64(), name);
        }
        OutputMode::Json => {
            emit(&Event::NodeReady {
                node: name,
                ts_ms: started.elapsed().as_millis(),
            });
        }
        OutputMode::Auto => unreachable!("auto resolved earlier"),
    }
}

fn report_finished(
    name: &str,
    outcome: &Outcome,
    duration: Duration,
    started: Instant,
    mode: OutputMode,
    pb: Option<&ProgressBar>,
) {
    match mode {
        OutputMode::Tui => {
            if let Some(pb) = pb {
                let suffix: String = match outcome {
                    Outcome::Succeeded => "✓ succeeded".to_string(),
                    Outcome::Failed(code) => format!("✗ failed (exit {code})"),
                    Outcome::Skipped => "⊘ skipped (dep failed)".to_string(),
                    Outcome::Stopped => "■ stopped (run finished)".to_string(),
                };
                pb.disable_steady_tick();
                pb.finish_with_message(suffix);
            }
        }
        OutputMode::Plain => {
            println!(
                "[{:>6.1}s] {} {}",
                started.elapsed().as_secs_f64(),
                name,
                outcome.label()
            );
        }
        OutputMode::Json => {
            let exit_code_value = match outcome {
                Outcome::Failed(c) => Some(*c),
                _ => None,
            };
            emit(&Event::NodeFinished {
                node: name,
                outcome: outcome.label(),
                exit_code: exit_code_value,
                duration_ms: duration.as_millis(),
            });
        }
        OutputMode::Auto => unreachable!("auto resolved earlier"),
    }
}

fn emit<T: Serialize>(event: &T) {
    if let Ok(line) = serde_json::to_string(event) {
        println!("{line}");
    }
}

fn exit_code(records: &BTreeMap<String, NodeRecord>) -> i32 {
    records
        .values()
        .map(|record| record.outcome.exit_contribution())
        .max()
        .unwrap_or(0)
}

struct Tally {
    succeeded: usize,
    failed: usize,
    skipped: usize,
    stopped: usize,
}

fn tally(records: &BTreeMap<String, NodeRecord>) -> Tally {
    let mut counts = Tally {
        succeeded: 0,
        failed: 0,
        skipped: 0,
        stopped: 0,
    };
    for record in records.values() {
        match record.outcome {
            Outcome::Succeeded => counts.succeeded += 1,
            Outcome::Failed(_) => counts.failed += 1,
            Outcome::Skipped => counts.skipped += 1,
            Outcome::Stopped => counts.stopped += 1,
        }
    }
    counts
}

fn print_summary(records: &BTreeMap<String, NodeRecord>, started: Instant) {
    let total = records.len();
    let Tally {
        succeeded,
        failed,
        skipped,
        stopped,
    } = tally(records);
    // Only mentioned when it happened, so a spec with no services in it reads
    // exactly as it did before services existed.
    let stopped_clause = if stopped > 0 {
        format!(", {stopped} stopped")
    } else {
        String::new()
    };
    eprintln!(
        "{total} task{plural}: {succeeded} succeeded, {failed} failed, \
         {skipped} skipped{stopped_clause} in {:.1}s",
        started.elapsed().as_secs_f64(),
        plural = if total == 1 { "" } else { "s" }
    );
    for (name, record) in records {
        eprintln!(
            "  {name}: {} ({:.1}s)",
            record.outcome.label(),
            record.duration.as_secs_f64()
        );
    }
    // Dump captured output from failed nodes so a CI log includes everything
    // needed to diagnose, since indicatif ate the live streams in TUI mode
    // and Stdio::piped() ate them everywhere else.
    for (name, record) in records {
        if matches!(record.outcome, Outcome::Failed(_))
            && (!record.stdout.is_empty() || !record.stderr.is_empty())
        {
            eprintln!("--- {name} stdout ---");
            eprintln!("{}", record.stdout.render().trim_end());
            eprintln!("--- {name} stderr ---");
            eprintln!("{}", record.stderr.render().trim_end());
        }
    }
}

fn emit_summary(records: &BTreeMap<String, NodeRecord>, started: Instant) {
    let Tally {
        succeeded,
        failed,
        skipped,
        stopped,
    } = tally(records);
    emit(&Event::Summary {
        total: records.len(),
        succeeded,
        failed,
        skipped,
        stopped,
        duration_ms: started.elapsed().as_millis(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(deps: &[&str]) -> NodeSpec {
        NodeSpec {
            command: vec!["true".into()],
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            env: BTreeMap::new(),
            timeout_secs: None,
            kind: Kind::Task,
            ready_when: None,
            ready_timeout_secs: default_ready_timeout_secs(),
            stdio: StdioMode::Capture,
            lifeline_fd: None,
        }
    }

    fn spec_of(nodes: &[(&str, &[&str])]) -> Spec {
        Spec {
            nodes: nodes
                .iter()
                .map(|(n, d)| ((*n).to_string(), node(d)))
                .collect(),
        }
    }

    fn record(outcome: Outcome) -> NodeRecord {
        NodeRecord {
            outcome,
            duration: Duration::ZERO,
            stdout: OutputTail::new(),
            stderr: OutputTail::new(),
        }
    }

    #[test]
    fn spec_round_trips_through_json() {
        let text = r#"{"nodes":{"a":{"command":["true"]},"b":{"command":["echo","x"],"depends_on":["a"],"env":{"K":"v"},"timeout_secs":30}}}"#;
        let spec: Spec = serde_json::from_str(text).unwrap();
        assert_eq!(spec.nodes.len(), 2);
        assert_eq!(spec.nodes["a"].command, vec!["true"]);
        assert!(spec.nodes["a"].depends_on.is_empty());
        assert!(spec.nodes["a"].env.is_empty());
        assert!(spec.nodes["a"].timeout_secs.is_none());
        assert_eq!(spec.nodes["b"].depends_on, vec!["a"]);
        assert_eq!(spec.nodes["b"].env.get("K").map(String::as_str), Some("v"));
        assert_eq!(spec.nodes["b"].timeout_secs, Some(30));
    }

    #[test]
    fn validate_rejects_missing_dependency() {
        let spec = spec_of(&[("a", &["ghost"])]);
        let err = validate(&spec).unwrap_err().to_string();
        assert!(
            err.contains("ghost"),
            "error should name the missing dep, got: {err}"
        );
        assert!(
            err.contains('a'),
            "error should name the offending node, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_empty_command() {
        let spec: Spec = serde_json::from_str(r#"{"nodes":{"a":{"command":[]}}}"#).unwrap();
        let err = validate(&spec).unwrap_err().to_string();
        assert!(
            err.contains("empty command"),
            "error should name the empty command, got: {err}"
        );
        assert!(
            err.contains('a'),
            "error should name the offending node, got: {err}"
        );
    }

    #[test]
    fn detect_cycle_catches_self_loop() {
        let spec = spec_of(&[("a", &["a"])]);
        let err = validate(&spec).unwrap_err().to_string();
        assert!(err.contains("cycle"), "expected cycle error, got: {err}");
    }

    #[test]
    fn detect_cycle_catches_indirect_cycle() {
        let spec = spec_of(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])]);
        let err = validate(&spec).unwrap_err().to_string();
        assert!(err.contains("cycle"), "expected cycle error, got: {err}");
    }

    #[test]
    fn validate_accepts_diamond() {
        let spec = spec_of(&[("a", &[]), ("b", &["a"]), ("c", &["a"]), ("d", &["b", "c"])]);
        validate(&spec).unwrap();
    }

    #[test]
    fn topological_order_places_root_first_and_sink_last() {
        let spec = spec_of(&[("a", &[]), ("b", &["a"]), ("c", &["a"]), ("d", &["b", "c"])]);
        let order = topological_order(&spec.nodes);
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert_eq!(pos("a"), 0);
        assert_eq!(pos("d"), 3);
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn topological_order_is_deterministic_for_independent_nodes() {
        let spec = spec_of(&[("c", &[]), ("a", &[]), ("b", &[])]);
        assert_eq!(
            topological_order(&spec.nodes),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn exit_code_zero_when_all_succeeded() {
        let mut records = BTreeMap::new();
        records.insert("a".into(), record(Outcome::Succeeded));
        records.insert("b".into(), record(Outcome::Succeeded));
        assert_eq!(exit_code(&records), 0);
    }

    #[test]
    fn exit_code_empty_is_zero() {
        let records = BTreeMap::new();
        assert_eq!(exit_code(&records), 0);
    }

    #[test]
    fn exit_code_propagates_single_failure() {
        let mut records = BTreeMap::new();
        records.insert("a".into(), record(Outcome::Failed(7)));
        assert_eq!(exit_code(&records), 7);
    }

    #[test]
    fn exit_code_picks_worst_failure_over_skipped() {
        let mut records = BTreeMap::new();
        records.insert("a".into(), record(Outcome::Succeeded));
        records.insert("b".into(), record(Outcome::Failed(3)));
        records.insert("c".into(), record(Outcome::Skipped));
        records.insert("d".into(), record(Outcome::Failed(9)));
        assert_eq!(exit_code(&records), 9);
    }

    #[test]
    fn exit_code_skipped_only_is_one() {
        let mut records = BTreeMap::new();
        records.insert("a".into(), record(Outcome::Skipped));
        assert_eq!(exit_code(&records), 1);
    }

    #[test]
    fn filter_only_keeps_named_nodes_and_drops_the_rest() {
        let mut spec = spec_of(&[("a", &[]), ("b", &["a"]), ("c", &[])]);
        filter_only(&mut spec, &["a".into(), "b".into()]).unwrap();
        let mut kept: Vec<&str> = spec.nodes.keys().map(String::as_str).collect();
        kept.sort_unstable();
        assert_eq!(kept, vec!["a", "b"]);
    }

    #[test]
    fn filter_only_errors_on_missing_name() {
        let mut spec = spec_of(&[("a", &[])]);
        let err = filter_only(&mut spec, &["ghost".into()])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("ghost"),
            "error should name the missing entry, got: {err}"
        );
    }

    #[test]
    fn filter_only_errors_when_kept_node_loses_its_dep() {
        let mut spec = spec_of(&[("a", &[]), ("b", &["a"])]);
        let err = filter_only(&mut spec, &["b".into()])
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("b -> a"),
            "error should show the dropped edge, got: {err}"
        );
    }

    #[test]
    fn already_gone_covers_the_zombie_group_darwin_reports_as_eperm() {
        assert!(ProcessGroupId::means_already_gone(&io::Error::from_raw_os_error(
            libc::ESRCH
        )));
        let eperm = io::Error::from_raw_os_error(libc::EPERM);
        assert_eq!(
            ProcessGroupId::means_already_gone(&eperm),
            cfg!(target_os = "macos"),
            "EPERM means an unreaped zombie group on Darwin and a real permission \
             failure everywhere else"
        );
        assert!(!ProcessGroupId::means_already_gone(
            &io::Error::from_raw_os_error(libc::EINVAL)
        ));
    }

    #[test]
    fn output_tail_keeps_everything_under_the_limit_verbatim() {
        let mut tail = OutputTail::new();
        tail.push("one\n".into());
        tail.push("two\n".into());
        assert_eq!(tail.render(), "one\ntwo\n");
        assert!(!tail.render().contains("dropped"));
    }

    #[test]
    fn output_tail_drops_the_oldest_lines_and_says_how_many() {
        let mut tail = OutputTail::new();
        for i in 0..(OutputTail::DEFAULT_LIMIT + 3) {
            tail.push(format!("line-{i}\n"));
        }
        let rendered = tail.render();
        assert!(rendered.starts_with("dag-runner: 3 earlier lines dropped"));
        assert!(!rendered.contains("line-0\n"));
        assert!(rendered.ends_with(&format!("line-{}\n", OutputTail::DEFAULT_LIMIT + 2)));
    }

    #[test]
    fn output_tail_singularises_one_dropped_line() {
        let mut tail = OutputTail::new();
        for i in 0..=OutputTail::DEFAULT_LIMIT {
            tail.push(format!("line-{i}\n"));
        }
        assert!(tail.render().starts_with("dag-runner: 1 earlier line dropped"));
    }

    #[test]
    fn exit_code_ignores_a_stopped_service() {
        // A service the runner took down did its job. Letting it colour the
        // exit code would make every successful supervised run look failed.
        let mut records = BTreeMap::new();
        records.insert("app".into(), record(Outcome::Succeeded));
        records.insert("server".into(), record(Outcome::Stopped));
        assert_eq!(exit_code(&records), 0);
    }

    #[test]
    fn service_kind_and_probes_round_trip_through_json() {
        let text = r#"{"nodes":{"s":{"command":["srv"],"kind":"service","stdio":"prefixed","lifeline_fd":3,"ready_timeout_secs":5,"ready_when":{"http":{"url":"http://x/","body_contains":"abc"}}}}}"#;
        let spec: Spec = serde_json::from_str(text).unwrap();
        let node = &spec.nodes["s"];
        assert!(node.is_service());
        assert_eq!(node.ready_timeout_secs, 5);
        assert_eq!(node.lifeline_fd, Some(3));
        assert_eq!(node.stdio, StdioMode::Prefixed);
        match node.ready_when.as_ref().expect("probe") {
            ReadyWhen::Http(probe) => {
                assert_eq!(probe.url, "http://x/");
                assert_eq!(probe.status, 200, "status defaults to 200");
                assert_eq!(probe.body_contains.as_deref(), Some("abc"));
            }
            other => panic!("expected an http probe, got {other:?}"),
        }
    }

    #[test]
    fn a_spec_written_before_services_existed_still_parses_as_tasks() {
        let text = r#"{"nodes":{"a":{"command":["true"]}}}"#;
        let spec: Spec = serde_json::from_str(text).unwrap();
        let node = &spec.nodes["a"];
        assert_eq!(node.kind, Kind::Task);
        assert!(node.ready_when.is_none());
        assert_eq!(node.stdio, StdioMode::Capture);
        assert!(node.lifeline_fd.is_none());
        validate(&spec).expect("a bare task spec is still valid");
    }

    #[test]
    fn truncate_for_spinner_preserves_short_strings() {
        assert_eq!(truncate_for_spinner("hello"), "hello");
    }

    #[test]
    fn truncate_for_spinner_clips_with_ellipsis() {
        let long: String = "x".repeat(200);
        let out = truncate_for_spinner(&long);
        assert!(out.chars().count() <= 80);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_for_spinner_is_char_safe() {
        // 100 four-byte emoji; byte-slicing would split a codepoint.
        let s: String = "🦀".repeat(100);
        let out = truncate_for_spinner(&s);
        assert!(out.chars().count() <= 80);
        assert!(out.ends_with('…'));
        // Round-trips as valid UTF-8 (no panic from char-count above).
        assert!(out.is_char_boundary(out.len()));
    }
}
