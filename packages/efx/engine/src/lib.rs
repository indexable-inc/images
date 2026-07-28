//! Effect engine: plan/apply over the content-addressed IR.
//!
//! The [`Journal`] is the state file *and* the cache: a mapping from
//! [`efx_ir::EffectId`] to recorded outputs. Because an effect's identity
//! already covers its executor, kind, and resolved inputs, "is this effect's
//! id in the journal?" is the entire memoization question — no separate
//! dirty-tracking. [`diff::plan`] answers it per effect; [`apply::apply`]
//! executes what is missing, level-parallel across independent effects, and
//! records the run so a later report can show what invalidated and why.

use std::collections::BTreeMap;
use std::fmt;

use efx_ir::Literal;
use snafu::Snafu;

pub mod apply;
pub mod diff;
pub mod journal;

pub use apply::{RunReport, apply};
pub use diff::{Decision, Orphan, PlanReport, Verdict, plan};
pub use journal::{Action, Journal, JournalEntry, RunEffect, RunRecord, Status};

/// Named output values produced by one effect execution.
pub type Outputs = BTreeMap<String, Literal>;

/// Every way an engine operation can fail before or outside executor code.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum EngineError {
    #[snafu(display("invalid plan"))]
    Plan { source: efx_ir::PlanError },

    #[snafu(display("read journal {path}"))]
    JournalRead {
        path: String,
        source: std::io::Error,
    },

    #[snafu(display("write journal {path}"))]
    JournalWrite {
        path: String,
        source: std::io::Error,
    },

    #[snafu(display("journal {path} is not valid journal JSON"))]
    JournalFormat {
        path: String,
        source: serde_json::Error,
    },

    #[snafu(display("effect `{effect}` needs executor `{executor}`, which is not registered"))]
    UnknownExecutor { effect: String, executor: String },
}

/// An executor failure, surfaced as the failed effect's reason.
#[derive(Debug)]
pub struct ExecuteError {
    message: String,
}

impl ExecuteError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ExecuteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ExecuteError {}

/// One effect, ready to run: all reference inputs resolved to literals.
pub struct ExecuteRequest {
    pub name: String,
    pub kind: String,
    pub inputs: BTreeMap<String, Literal>,
}

/// Something that can perform one kind of effect.
pub trait Executor: Send + Sync {
    /// Performs the effect and returns its named outputs.
    ///
    /// # Errors
    ///
    /// Returns [`ExecuteError`] when the side effect cannot be performed;
    /// the engine records the effect as failed and skips its dependents.
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError>;
}

/// Executor lookup by id.
#[derive(Default)]
pub struct Registry {
    executors: BTreeMap<String, Box<dyn Executor>>,
}

impl Registry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, id: impl Into<String>, executor: Box<dyn Executor>) {
        self.executors.insert(id.into(), executor);
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&dyn Executor> {
        self.executors.get(id).map(AsRef::as_ref)
    }
}
