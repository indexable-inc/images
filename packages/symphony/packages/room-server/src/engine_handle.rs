//! Static dispatch over the two known engines.
//!
//! The [`Engine`] trait uses async methods, which desugar to
//! return-position `impl Future` (RPITIT). That makes the trait not
//! object-safe, so `Box<dyn Engine>` is not available without pulling
//! in `async-trait` and paying a boxed-future allocation per call.
//! With exactly two engines (codex, claude) an enum that forwards to
//! each adapter is the cleaner choice: it keeps the futures unboxed,
//! adds no dependency, and the exhaustiveness check flags any engine
//! added later that forgets a method. A new engine adds one variant and
//! the compiler points at every missing arm.

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::broadcast;

use crate::{
    engine::{
        Engine, EngineAnswer, EngineEvent, EngineKind, RequestId, TurnHandle, TurnRequest,
        TurnStatus,
    },
    engine_claude::ClaudeEngine,
    engine_codex::CodexEngine,
};

/// One of the concrete engine adapters, dispatched statically. Holds an
/// `Arc` so clones share the same underlying subprocess and event
/// channel.
#[derive(Clone)]
pub enum EngineHandle {
    Codex(Arc<CodexEngine>),
    Claude(Arc<ClaudeEngine>),
}

impl EngineHandle {
    pub fn kind(&self) -> EngineKind {
        match self {
            Self::Codex(e) => e.kind(),
            Self::Claude(e) => e.kind(),
        }
    }

    pub async fn start_turn(&self, turn: TurnRequest) -> Result<TurnHandle> {
        match self {
            Self::Codex(e) => e.start_turn(turn).await,
            Self::Claude(e) => e.start_turn(turn).await,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        match self {
            Self::Codex(e) => e.subscribe(),
            Self::Claude(e) => e.subscribe(),
        }
    }

    pub async fn answer(&self, req_id: RequestId, answer: EngineAnswer) -> Result<()> {
        match self {
            Self::Codex(e) => e.answer(req_id, answer).await,
            Self::Claude(e) => e.answer(req_id, answer).await,
        }
    }

    pub async fn interrupt(&self, turn: &TurnHandle) -> Result<()> {
        match self {
            Self::Codex(e) => e.interrupt(turn).await,
            Self::Claude(e) => e.interrupt(turn).await,
        }
    }

    pub async fn status(&self, turn: &TurnHandle) -> Result<TurnStatus> {
        match self {
            Self::Codex(e) => e.status(turn).await,
            Self::Claude(e) => e.status(turn).await,
        }
    }

    pub async fn wait_for_exit(&self) {
        match self {
            Self::Codex(e) => e.wait_for_exit().await,
            Self::Claude(e) => e.wait_for_exit().await,
        }
    }
}

/// The engines a room-server instance hosts, keyed by kind. The HTTP
/// and runtime layers look an engine up by the `engine` field on a
/// [`TurnRequest`] and never name a concrete adapter. A deploy that
/// lacks a binary (no `codex` on PATH, no `ANTHROPIC_API_KEY`) simply
/// omits that entry, and a turn for a missing engine returns a clear
/// "engine not configured" error instead of a panic.
#[derive(Clone, Default)]
pub struct EngineRegistry {
    codex: Option<EngineHandle>,
    claude: Option<EngineHandle>,
}

impl EngineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_codex(mut self, engine: Arc<CodexEngine>) -> Self {
        self.codex = Some(EngineHandle::Codex(engine));
        self
    }

    pub fn with_claude(mut self, engine: Arc<ClaudeEngine>) -> Self {
        self.claude = Some(EngineHandle::Claude(engine));
        self
    }

    /// Look up the engine that should run a turn of the given kind.
    pub fn get(&self, kind: EngineKind) -> Option<&EngineHandle> {
        match kind {
            EngineKind::Codex => self.codex.as_ref(),
            EngineKind::Claude => self.claude.as_ref(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.codex.is_none() && self.claude.is_none()
    }
}
