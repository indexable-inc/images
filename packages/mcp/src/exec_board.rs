//! Publish every Python execution to the dashboard as an `exec` pane.
//!
//! Each `python_eval`/`python_exec` becomes one card: a box showing the run's
//! captured stdout and stderr, with the source behind it. The board binds one
//! process-global producer socket (the same discovery mechanism the `tui`
//! terminals use) and republishes its full pane set whenever a call starts or
//! finishes, so the standalone `dashboard` aggregator renders them live and the
//! recorder persists them for replay.
//!
//! The producer is a best-effort convenience, mirroring the `tui` auto-publish:
//! if the socket cannot bind (no writable discovery directory, no runtime), exec
//! panes are disabled and the Python tools keep working unchanged. A bind
//! failure is logged to stderr so the loss is observable.

#![allow(
    clippy::significant_drop_tightening,
    reason = "lock the pane set, reconcile it under the guard, then drop the guard before publishing: the same guard-then-extract pattern dashboard-core allows for its shared locks"
)]

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use dashboard_core::{ExecTraceLine, ExecView, Pane, PaneSink, Publisher, View, socket_path};
use parking_lot::Mutex;
use serde_json::Value;

/// Most recent executions kept on the live board. Each call is its own pane, so
/// without a cap a long agent session would grow the producer's snapshot without
/// bound; older cards scroll off the live board but survive in the recording.
const MAX_PANES: usize = 100;

/// The process-global exec board, bound on first use. `None` once a bind was
/// attempted and failed, so the failure is logged once rather than per call.
static BOARD: OnceLock<Option<ExecBoard>> = OnceLock::new();

/// The shared exec board: bind the producer once and reuse it for the life of
/// the process. Returns `None` when the producer could not bind.
pub fn global() -> Option<&'static ExecBoard> {
    BOARD
        .get_or_init(|| match ExecBoard::bind() {
            Ok(board) => Some(board),
            Err(error) => {
                eprintln!("ix-mcp: dashboard exec panes disabled ({error:#})");
                None
            }
        })
        .as_ref()
}

/// One process's exec panes plus the producer socket that streams them.
pub struct ExecBoard {
    // Kept alive for the process so the socket stays bound; never mutated after
    // bind (it lives until exit), so it needs no lock.
    _publisher: Publisher,
    sink: PaneSink,
    panes: Mutex<Vec<Pane>>,
    seq: AtomicU64,
}

impl ExecBoard {
    /// Bind the producer socket on the current tokio runtime.
    fn bind() -> Result<Self> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| anyhow!("no tokio runtime to bind the dashboard producer on"))?;
        let publisher = Publisher::bind(socket_path(), &runtime)
            .map_err(|source| anyhow!("bind dashboard producer: {source}"))
            .context("dashboard producer")?;
        let sink = publisher.sink();
        Ok(Self {
            _publisher: publisher,
            sink,
            panes: Mutex::new(Vec::new()),
            seq: AtomicU64::new(0),
        })
    }

    /// Record the start of a call: add a running pane and publish. Returns the
    /// pane id to pass back to [`finish`](Self::finish).
    pub fn start(&self, session: &str, lang: &str, op_label: &str, source: String) -> String {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let id = format!("{session}/{n}");
        let mut pane = Pane::exec(
            &id,
            ExecView {
                source,
                lang: lang.to_owned(),
                stdout: String::new(),
                stderr: String::new(),
                result: String::new(),
                running: true,
                ok: None,
                trace: Vec::new(),
            },
        );
        pane.subtitle = format!("{op_label} · {session}");
        {
            let mut panes = self.panes.lock();
            panes.push(pane);
            // Trim to the cap, but never drop a still-running execution: `finish`
            // only updates a pane that is still present, so pruning a running one
            // would lose its captured output. Drop the oldest finished panes
            // first; in-flight panes are bounded by concurrent calls, so keeping
            // them cannot grow the set without bound.
            while panes.len() > MAX_PANES {
                match panes.iter().position(|pane| !is_running(pane)) {
                    Some(oldest_finished) => {
                        panes.remove(oldest_finished);
                    }
                    None => break,
                }
            }
        }
        self.publish();
        id
    }

    /// Record a finished call from the worker response: fill in the captured
    /// output and flip the pane out of its running state.
    pub fn finish_from_response(&self, id: &str, response: &Value) {
        let field = |key: &str| {
            response
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        // A Python exception comes back as a successful round-trip with
        // `ok: false`; absence of the field means a control op with no flag, so
        // treat it as success.
        let ok = response.get("ok").and_then(Value::as_bool).unwrap_or(true);
        // Inline-trace execution: the worker pairs each stdout chunk with the
        // source line that printed it (see python_worker.py). Absent for an older
        // worker or a run with no line-attributed output.
        let trace = response
            .get("trace")
            .and_then(|value| serde_json::from_value::<Vec<ExecTraceLine>>(value.clone()).ok())
            .unwrap_or_default();
        self.finish(id, field("stdout"), field("stderr"), field("result"), ok, trace);
    }

    /// Record a transport failure (timeout, cancel, worker death): the call
    /// produced no worker response, so surface the error as the pane's stderr.
    pub fn finish_error(&self, id: &str, error: &str) {
        self.finish(id, String::new(), error.to_owned(), String::new(), false, Vec::new());
    }

    fn finish(
        &self,
        id: &str,
        stdout: String,
        stderr: String,
        result: String,
        ok: bool,
        trace: Vec<ExecTraceLine>,
    ) {
        {
            let mut panes = self.panes.lock();
            if let Some(View::Exec(view)) = panes
                .iter_mut()
                .find(|pane| pane.id == id)
                .map(|pane| &mut pane.view)
            {
                view.stdout = stdout;
                view.stderr = stderr;
                view.result = result;
                view.running = false;
                view.ok = Some(ok);
                view.trace = trace;
            }
        }
        self.publish();
    }

    fn publish(&self) {
        let panes = self.panes.lock().clone();
        self.sink.publish(&panes);
    }
}

/// Whether an exec pane's call is still in flight.
const fn is_running(pane: &Pane) -> bool {
    if let View::Exec(view) = &pane.view {
        view.running
    } else {
        false
    }
}
