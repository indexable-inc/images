//! Run a JSON-described DAG of commands with inline progress.
//!
//! The spec is a flat map of nodes, each with an argv `command`, an
//! optional `depends_on` list, an optional `env` overlay, and an
//! optional `timeout_secs` wall-clock limit. Nodes whose deps have
//! completed run as soon as they are unblocked, so the layout of the
//! graph determines how much parallelism is achievable; there is no
//! notion of "levels".
//!
//! Output modes:
//! - `auto` (default): TUI on a TTY, plain otherwise.
//! - `tui`: indicatif `MultiProgress` with one inline spinner per node.
//! - `plain`: line-buffered "started" / "finished" lines, no spinners.
//! - `json`: NDJSON event stream plus a final `summary` record.
//!
//! Exit code reflects the worst node outcome: zero if every node succeeded,
//! the worst non-zero command exit code otherwise, or 1 if any node was
//! skipped because a dep failed. Ctrl-C cancels every running child
//! (SIGTERM, brief grace, SIGKILL) and forces exit 130; a second Ctrl-C
//! hard-exits immediately.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::Mutex;

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
struct Spec {
    nodes: HashMap<String, NodeSpec>,
}

#[derive(Deserialize, Clone)]
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
}

#[derive(Clone, Debug)]
enum Outcome {
    Succeeded,
    Failed(i32),
    Skipped,
}

impl Outcome {
    const fn label(&self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed(_) => "failed",
            Self::Skipped => "skipped",
        }
    }
}

struct NodeRecord {
    outcome: Outcome,
    duration: Duration,
    stdout: String,
    stderr: String,
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum Event<'a> {
    NodeStarted {
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
        duration_ms: u128,
    },
}

enum CycleColor {
    Gray,
    Black,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let text = std::fs::read_to_string(&args.spec)
        .with_context(|| format!("reading spec: {}", args.spec.display()))?;
    let mut spec: Spec = serde_json::from_str(&text).context("parsing spec JSON")?;

    if !args.only.is_empty() {
        filter_only(&mut spec, &args.only)?;
    }

    validate(&spec)?;

    let mode = resolve_mode(args.output);
    let started = Instant::now();

    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    spawn_cancel_listener(cancel_tx);

    let records = run(spec, mode, started, cancel_rx.clone()).await?;

    if matches!(mode, OutputMode::Json) {
        emit_summary(&records, started);
    } else {
        print_summary(&records, started);
    }

    // A cancellation always exits 130, even if every node managed to finish
    // (succeed, fail, or skip) before being killed. Callers distinguish
    // "cancelled by the operator" from "ran to completion" via this code.
    if *cancel_rx.borrow() {
        std::process::exit(130);
    }
    std::process::exit(exit_code(&records));
}

/// Background task: first Ctrl-C broadcasts cancellation; second hard-exits.
fn spawn_cancel_listener(cancel_tx: tokio::sync::watch::Sender<bool>) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_err() {
            return;
        }
        eprintln!(
            "dag-runner: SIGINT received, cancelling running nodes (Ctrl-C again to hard-exit)"
        );
        let _ = cancel_tx.send(true);
        if tokio::signal::ctrl_c().await.is_ok() {
            eprintln!("dag-runner: second SIGINT, hard-exiting");
            std::process::exit(130);
        }
    });
}

fn resolve_mode(requested: OutputMode) -> OutputMode {
    match requested {
        OutputMode::Auto => {
            if std::io::stdout().is_terminal() {
                OutputMode::Tui
            } else {
                OutputMode::Plain
            }
        }
        m => m,
    }
}

fn validate(spec: &Spec) -> Result<()> {
    for (name, node) in &spec.nodes {
        if node.command.is_empty() {
            bail!("node {name} has empty command");
        }
        for dep in &node.depends_on {
            if !spec.nodes.contains_key(dep) {
                bail!("node {name} depends on unknown node {dep}");
            }
        }
    }
    detect_cycle(&spec.nodes)?;
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

type SharedOutcome = Shared<BoxFuture<'static, Outcome>>;

async fn run(
    spec: Spec,
    mode: OutputMode,
    started: Instant,
    cancel_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<BTreeMap<String, NodeRecord>> {
    let multi = matches!(mode, OutputMode::Tui).then(MultiProgress::new);
    let records: Arc<Mutex<BTreeMap<String, NodeRecord>>> = Arc::new(Mutex::new(BTreeMap::new()));

    let order = topological_order(&spec.nodes);
    let mut futs: HashMap<String, SharedOutcome> = HashMap::new();

    for name in &order {
        let node = spec.nodes[name].clone();
        let name_owned = name.clone();
        let dep_futs: Vec<SharedOutcome> =
            node.depends_on.iter().map(|d| futs[d].clone()).collect();
        let pb = multi.as_ref().map(|m| make_spinner(m, &name_owned));
        let records_for_task = records.clone();
        let cancel_for_task = cancel_rx.clone();

        let fut = async move {
            for dep in &dep_futs {
                let _ = dep.clone().await;
            }

            let any_dep_bad = {
                let guard = records_for_task.lock().await;
                node.depends_on
                    .iter()
                    .any(|d| !matches!(guard.get(d).map(|r| &r.outcome), Some(Outcome::Succeeded)))
            };

            if any_dep_bad {
                let duration = Duration::ZERO;
                report_finished(
                    &name_owned,
                    &Outcome::Skipped,
                    duration,
                    started,
                    mode,
                    pb.as_ref(),
                );
                records_for_task.lock().await.insert(
                    name_owned.clone(),
                    NodeRecord {
                        outcome: Outcome::Skipped,
                        duration,
                        stdout: String::new(),
                        stderr: String::new(),
                    },
                );
                return Outcome::Skipped;
            }

            report_started(&name_owned, started, mode, pb.as_ref());
            let node_started = Instant::now();
            let (outcome, stdout, stderr) = run_command(&node, pb.as_ref(), cancel_for_task).await;
            let duration = node_started.elapsed();
            report_finished(&name_owned, &outcome, duration, started, mode, pb.as_ref());

            records_for_task.lock().await.insert(
                name_owned.clone(),
                NodeRecord {
                    outcome: outcome.clone(),
                    duration,
                    stdout,
                    stderr,
                },
            );
            outcome
        }
        .boxed()
        .shared();

        futs.insert(name.clone(), fut);
    }

    let handles: Vec<_> = futs.values().cloned().map(tokio::spawn).collect();
    for handle in handles {
        let _ = handle.await;
    }

    if let Some(multi) = multi {
        let _ = multi.clear();
    }

    let final_records = std::mem::take(&mut *records.lock().await);
    Ok(final_records)
}

fn make_spinner(multi: &MultiProgress, name: &str) -> ProgressBar {
    let pb = multi.add(ProgressBar::new_spinner());
    // The template uses indicatif's own substitution syntax, which clippy's
    // literal-string-with-formatting-args lint mistakes for a `format!`
    // template; allow it on this one call.
    #[allow(clippy::literal_string_with_formatting_args)]
    let style =
        ProgressStyle::with_template("{spinner:.cyan} {prefix:.bold} {wide_msg} {elapsed:.dim}")
            .expect("static template")
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ");
    pb.set_style(style);
    pb.set_prefix(name.to_string());
    pb.set_message("pending");
    pb
}

async fn run_command(
    node: &NodeSpec,
    pb: Option<&ProgressBar>,
    mut cancel_rx: tokio::sync::watch::Receiver<bool>,
) -> (Outcome, String, String) {
    // If cancellation already fired (a later-spawning node sees the bit
    // before it ever reaches its select!), skip the work entirely.
    if *cancel_rx.borrow() {
        return (
            Outcome::Failed(130),
            String::new(),
            "dag-runner: cancelled\n".into(),
        );
    }

    let mut cmd = Command::new(&node.command[0]);
    cmd.args(&node.command[1..])
        .envs(&node.env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // If we panic or drop the future for any reason, don't leak a child
        // into the surrounding shell.
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                Outcome::Failed(127),
                String::new(),
                format!("failed to spawn: {e}\n"),
            );
        }
    };

    let stdout_pipe = child.stdout.take().expect("stdout piped");
    let stderr_pipe = child.stderr.take().expect("stderr piped");
    let stdout_task = tokio::spawn(tee_lines(stdout_pipe, pb.cloned()));
    let stderr_task = tokio::spawn(tee_lines(stderr_pipe, pb.cloned()));

    let completion = tokio::select! {
        biased;
        () = wait_for_cancel(&mut cancel_rx) => Completion::Cancelled,
        () = maybe_timeout(node.timeout_secs) => Completion::TimedOut,
        res = child.wait() => match res {
            Ok(status) => {
                if status.success() {
                    Completion::Succeeded
                } else {
                    Completion::Failed(status.code().unwrap_or(1))
                }
            }
            Err(e) => Completion::WaitFailed(e.to_string()),
        },
    };

    let mut extra_stderr = String::new();
    let outcome = match completion {
        Completion::Succeeded => Outcome::Succeeded,
        Completion::Failed(code) => Outcome::Failed(code),
        Completion::WaitFailed(msg) => {
            use std::fmt::Write;
            let _ = writeln!(extra_stderr, "wait failed: {msg}");
            Outcome::Failed(1)
        }
        Completion::TimedOut => {
            use std::fmt::Write;
            // Safe to unwrap: only the timeout arm produces TimedOut, and
            // maybe_timeout only resolves when timeout_secs is Some.
            let secs = node
                .timeout_secs
                .expect("timeout arm requires timeout_secs");
            terminate_child(&mut child).await;
            let _ = writeln!(extra_stderr, "dag-runner: node timed out after {secs}s");
            Outcome::Failed(124)
        }
        Completion::Cancelled => {
            terminate_child(&mut child).await;
            extra_stderr.push_str("dag-runner: cancelled\n");
            Outcome::Failed(130)
        }
    };

    let stdout = stdout_task.await.unwrap_or_default();
    let mut stderr = stderr_task.await.unwrap_or_default();
    stderr.push_str(&extra_stderr);
    (outcome, stdout, stderr)
}

enum Completion {
    Succeeded,
    Failed(i32),
    WaitFailed(String),
    TimedOut,
    Cancelled,
}

/// Resolves the first time the cancellation flag flips from false to true.
/// Guards against the case where the watch sender was dropped (returns
/// without firing) so the caller can keep racing other arms.
async fn wait_for_cancel(rx: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            // Sender dropped without ever cancelling. Park forever so the
            // outer select! falls through to whichever arm wins.
            std::future::pending::<()>().await;
        }
    }
}

/// Resolves after `secs` seconds when set, otherwise blocks forever. Used as
/// the timeout arm of a `tokio::select!`: pairing it with `child.wait()`
/// lets the wait win when no timeout was requested.
async fn maybe_timeout(secs: Option<u64>) {
    match secs {
        Some(s) => tokio::time::sleep(Duration::from_secs(s)).await,
        None => std::future::pending::<()>().await,
    }
}

/// `SIGTERM` the child, wait a brief grace period for it to exit cleanly,
/// then `SIGKILL` if it's still alive. `tokio::process::Child::start_kill`
/// is `SIGKILL` only; sending `SIGTERM` first gives well-behaved children
/// a chance to flush state.
async fn terminate_child(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        // Safety: `pid` was just returned by the OS for a child we own and
        // have not yet reaped, and `SIGTERM` is a valid signal number.
        unsafe {
            libc::kill(pid.cast_signed(), libc::SIGTERM);
        }
    }
    let grace = tokio::time::sleep(Duration::from_millis(500));
    tokio::pin!(grace);
    tokio::select! {
        () = &mut grace => {
            let _ = child.start_kill();
        }
        _ = child.wait() => return,
    }
    let _ = child.wait().await;
}

/// Read `stream` line-by-line, returning the full captured text and, when a
/// spinner is wired up, updating its message with the most recent non-empty
/// line so a long-running node looks alive instead of just ticking elapsed.
async fn tee_lines(stream: impl AsyncRead + Unpin, pb: Option<ProgressBar>) -> String {
    let mut reader = BufReader::new(stream);
    let mut captured = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                captured.push_str(&line);
                if let Some(pb) = &pb {
                    let trimmed = line.trim_end_matches(['\n', '\r']);
                    if !trimmed.is_empty() {
                        pb.set_message(truncate_for_spinner(trimmed));
                    }
                }
            }
        }
    }
    captured
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
    let mut worst = 0i32;
    for record in records.values() {
        let code = match &record.outcome {
            Outcome::Succeeded => 0,
            Outcome::Failed(c) => *c,
            Outcome::Skipped => 1,
        };
        if code > worst {
            worst = code;
        }
    }
    worst
}

fn print_summary(records: &BTreeMap<String, NodeRecord>, started: Instant) {
    let total = records.len();
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    for record in records.values() {
        match record.outcome {
            Outcome::Succeeded => succeeded += 1,
            Outcome::Failed(_) => failed += 1,
            Outcome::Skipped => skipped += 1,
        }
    }
    eprintln!(
        "{total} task{plural}: {succeeded} succeeded, {failed} failed, {skipped} skipped in {:.1}s",
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
            eprintln!("{}", record.stdout.trim_end());
            eprintln!("--- {name} stderr ---");
            eprintln!("{}", record.stderr.trim_end());
        }
    }
}

fn emit_summary(records: &BTreeMap<String, NodeRecord>, started: Instant) {
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    for record in records.values() {
        match record.outcome {
            Outcome::Succeeded => succeeded += 1,
            Outcome::Failed(_) => failed += 1,
            Outcome::Skipped => skipped += 1,
        }
    }
    emit(&Event::Summary {
        total: records.len(),
        succeeded,
        failed,
        skipped,
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
            stdout: String::new(),
            stderr: String::new(),
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
