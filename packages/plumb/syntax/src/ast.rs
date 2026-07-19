//! Spanned AST for the plumb grammar (a strict bash subset).
//!
//! Everything the parser accepts pastes into bash unchanged and means the
//! same thing there; anything else is a loud [`crate::ParseError`], never a
//! silent reinterpretation.

/// Byte range into the original source string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

impl Span {
    /// Slice `src` to this span.
    #[must_use]
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        src.get(self.start..self.end).unwrap_or("")
    }
}

/// One component of a [`Word`] after quote processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// Literal text. `quoted` records whether it came from inside quotes or
    /// an escape, which makes it inert to glob expansion.
    Text {
        /// The literal text, with quotes and escapes already resolved.
        text: String,
        /// True when the text originated inside quotes or a backslash escape.
        quoted: bool,
    },
    /// `$NAME` / `${NAME}` / `$?` variable reference, or a run reference:
    /// the terse `${o[7]}` / `${o[7][0]}` forms and the structured
    /// `${runs[7].stages[0].stdout}` paths (braced form only).
    Var {
        /// Variable name (`?` for the last-status special).
        name: String,
        /// Path segments after the name; empty for a plain reference.
        path: Vec<PathSeg>,
        /// Location of the reference in the source.
        span: Span,
    },
    /// `$(...)` command substitution holding the nested program.
    CommandSub {
        /// The parsed program between the parentheses.
        program: Program,
        /// Location of the substitution in the source.
        span: Span,
    },
}

/// One step of a run-reference path: `[7]` or `.stdout`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    /// `[i]`: negative values count back from the latest.
    Index(i64),
    /// `.name`
    Field(String),
}

/// A single shell word: the concatenation of its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    /// Components in source order.
    pub parts: Vec<Part>,
    /// Location of the whole word.
    pub span: Span,
}

/// `NAME=value` assignment (command prefix or standalone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assign {
    /// Variable name.
    pub name: String,
    /// Assigned value (empty word for `NAME=`).
    pub value: Word,
    /// Location of the whole assignment.
    pub span: Span,
}

/// Redirection operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirOp {
    /// `> file`
    OutTrunc,
    /// `>> file`
    OutAppend,
    /// `2> file`
    ErrTrunc,
    /// `2>> file`
    ErrAppend,
    /// `&> file` (stdout and stderr)
    BothTrunc,
    /// `&>> file`
    BothAppend,
    /// `< file`
    In,
    /// `2>&1`
    ErrToOut,
    /// `1>&2`
    OutToErr,
}

/// One redirection attached to a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    /// Which operator.
    pub op: RedirOp,
    /// File target; `None` for the fd-duplication forms (`2>&1`, `1>&2`).
    pub target: Option<Word>,
    /// Location of the redirection.
    pub span: Span,
}

/// A simple command: optional assignment prefix, argv words, redirections.
/// A command with assignments and no words assigns shell variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// `NAME=value` prefixes (or the whole command when `words` is empty).
    pub assigns: Vec<Assign>,
    /// Argv words (element 0 is the program).
    pub words: Vec<Word>,
    /// Redirections in source order.
    pub redirects: Vec<Redirect>,
    /// Location of the whole command.
    pub span: Span,
}

/// `a | b | c`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pipeline {
    /// Stages, left to right.
    pub commands: Vec<Command>,
    /// Location of the whole pipeline.
    pub span: Span,
}

/// Connector between pipelines in an and-or chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connector {
    /// `&&`: run only when the previous pipeline succeeded.
    And,
    /// `||`: run only when the previous pipeline failed.
    Or,
}

/// One `&& pipeline` / `|| pipeline` continuation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndOrTail {
    /// The connector before the pipeline.
    pub connector: Connector,
    /// The pipeline it guards.
    pub pipeline: Pipeline,
}

/// `p1 && p2 || p3` chain, evaluated left to right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndOr {
    /// The first pipeline.
    pub first: Pipeline,
    /// The chained continuations.
    pub rest: Vec<AndOrTail>,
    /// Location of the whole chain.
    pub span: Span,
}

/// A top-level item: an and-or chain, optionally backgrounded with `&`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The chain to run.
    pub and_or: AndOr,
    /// True when terminated by `&`.
    pub background: bool,
}

/// A parsed source string: items separated by `;` or newlines.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Program {
    /// Items in source order.
    pub items: Vec<Item>,
}
