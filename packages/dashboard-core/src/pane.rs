//! Wire types and discovery paths shared by every producer ([`crate::publish`])
//! and the aggregator ([`crate::dashboard`]).
//!
//! A producer streams [`ProducerSnapshot`]s over a unix socket; the aggregator
//! folds them into one document keyed by `producer`. Both halves agree on these
//! shapes and on where the sockets live ([`discovery_dir`]), so neither side
//! reaches into the other.
//!
//! The unit is a [`Pane`]: a titled card whose body is one [`View`]. A view is a
//! tagged union over rendering strategies, from the bandwidth-cheap ANSI
//! [`TerminalView`] with a built-in renderer, through a producer-defined
//! [`HtmlView`], the first-class [`ExecView`] for a captured process run, to
//! structured [`DataView`] JSON rendered by a named frontend renderer. The
//! aggregator never learns what a pane *means*; it stores the view and the
//! browser renders it by `kind`. A new first-class resource adds a typed `View`
//! variant and a native renderer; a user-defined one reuses `Html`/`Data` with
//! no aggregator change.
//!
//! A view declares its storage shape to the aggregator as a set of scalar meta
//! fields and a set of named large-text fields (see the projection in
//! [`crate::dashboard`]). Every pane additionally carries a creation timestamp,
//! stamped once by the aggregator when the pane first appears, so the canvas can
//! show each resource's age uniformly with no producer opt-in.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One unit on the dashboard canvas.
///
/// `id` is unique within its producer; the aggregator namespaces it by producer
/// for a global key. Any resource the MCP exposes — a PTY terminal, a VM screen,
/// a future thing — publishes itself as a `Pane`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pane {
    /// Stable per-producer id (a UUID, a slug, whatever the producer keys on).
    pub id: String,
    /// Human-facing card title (a command, a hostname, a label).
    pub title: String,
    /// Optional one-line subtitle (args, a status, a URL). Empty hides it.
    #[serde(default)]
    pub subtitle: String,
    /// The pane body, tagged by `kind` on the wire.
    pub view: View,
}

impl Pane {
    /// A terminal pane from a [`TerminalView`], titling it with the command.
    #[must_use]
    pub fn terminal(id: impl Into<String>, view: TerminalView) -> Self {
        let title = view.command.clone();
        let subtitle = view.args.clone();
        Self {
            id: id.into(),
            title,
            subtitle,
            view: View::Terminal(view),
        }
    }

    /// An HTML pane: the producer ships its own UI, mounted sandboxed.
    #[must_use]
    pub fn html(id: impl Into<String>, title: impl Into<String>, html: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: String::new(),
            view: View::Html(HtmlView { html: html.into() }),
        }
    }

    /// An execution pane: one captured process run, titled with the first
    /// non-empty line of its source so the card reads like a command. The
    /// producer publishes a `running` view when the call starts and replaces it
    /// with the finished view when it returns.
    #[must_use]
    pub fn exec(id: impl Into<String>, view: ExecView) -> Self {
        let title = exec_title(&view.source);
        Self {
            id: id.into(),
            title,
            subtitle: String::new(),
            view: View::Exec(view),
        }
    }

    /// A data pane: structured JSON rendered by the named frontend `renderer`,
    /// falling back to a generic tree when the name is unknown.
    #[must_use]
    pub fn data(
        id: impl Into<String>,
        title: impl Into<String>,
        renderer: impl Into<String>,
        data: serde_json::Value,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: String::new(),
            view: View::Data(DataView {
                renderer: renderer.into(),
                data,
            }),
        }
    }
}

/// A pane's body: a tagged union over rendering strategies.
///
/// Serialized with an internal `kind` tag (`"terminal"`, `"html"`, `"exec"`,
/// `"data"`) so the dashboard can store and the browser can render each body
/// without a separate schema negotiation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum View {
    /// A live terminal screen as minimal ANSI SGR, with cursor and exit state.
    Terminal(TerminalView),
    /// A self-contained HTML document the producer renders itself.
    Html(HtmlView),
    /// One captured process run: its source, stdout, stderr, and result.
    Exec(ExecView),
    /// Structured JSON plus the name of a frontend renderer.
    Data(DataView),
}

impl View {
    /// The wire tag for this view, matching the `kind` serde discriminant.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Terminal(_) => "terminal",
            Self::Html(_) => "html",
            Self::Exec(_) => "exec",
            Self::Data(_) => "data",
        }
    }
}

/// One terminal's rendered state at a single poll tick.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TerminalView {
    /// The command that was spawned.
    pub command: String,
    /// Positional arguments, space-joined for display.
    pub args: String,
    /// Terminal height in rows.
    pub rows: u16,
    /// Terminal width in columns.
    pub cols: u16,
    /// Whether the child is still running.
    pub alive: bool,
    /// The visible screen, rows newline-joined, with minimal ANSI SGR runs
    /// encoding per-cell color and attributes. The dashboard parses the SGR
    /// back into styled spans; a plain reader still sees the text.
    pub screen: String,
    // These fields were added after the first wire shape. `#[serde(default)]`
    // keeps a mixed-version dashboard working: a producer built before this
    // change streams views without them, and the aggregator drops a snapshot
    // whose JSON fails to parse, so without defaults those older producers'
    // panes would silently vanish.
    /// Cursor row in viewport cell coordinates (0-based, top first).
    #[serde(default)]
    pub cursor_row: u16,
    /// Cursor column in viewport cell coordinates (0-based, left first).
    #[serde(default)]
    pub cursor_col: u16,
    /// Whether the screen is showing its cursor (the inverse of `CSI ?25l`).
    #[serde(default)]
    pub cursor_visible: bool,
    /// The cursor shape token: `"block"`, `"underline"`, or `"bar"`.
    #[serde(default)]
    pub cursor_shape: String,
    /// The child's exit code when it has exited with one, else `None` (still
    /// running, or terminated by a signal).
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// A producer-rendered HTML body.
///
/// The dashboard mounts it in a sandboxed frame and records it in a Loro text
/// container, so the body diffs incrementally and its history replays like any
/// other pane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HtmlView {
    /// A self-contained HTML fragment or document.
    pub html: String,
}

/// One process execution's captured state: the source behind it and the output
/// it produced.
///
/// A producer publishes a `running` view (empty output, `ok` absent) when the
/// call starts and replaces it with the finished view when the call returns, so
/// the card animates from "running" to its result. The MCP Python tools are the
/// first producer: each `python_exec`/`python_eval` becomes one of these, and
/// because the worker captures output at the file-descriptor level, a spawned
/// subprocess's `stdout`/`stderr` (e.g. an `echo`) lands here too.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecView {
    /// The source the producer ran: the statements (`exec`) or the expression
    /// (`eval`). Shown behind the output when the card is opened.
    pub source: String,
    /// The language of `source`, for the frontend's syntax label. `"python"`
    /// today; an empty value renders unlabelled.
    #[serde(default)]
    pub lang: String,
    /// Captured standard output, including any spawned subprocess's, in order.
    #[serde(default)]
    pub stdout: String,
    /// Captured standard error, including any spawned subprocess's.
    #[serde(default)]
    pub stderr: String,
    /// The evaluated expression's `repr` for an `eval`, else empty.
    #[serde(default)]
    pub result: String,
    /// Whether the call is still in flight: `true` until it returns.
    pub running: bool,
    /// Whether a finished call succeeded: `None` while running, `Some(false)` on
    /// a Python exception or a transport failure (timeout, cancel).
    #[serde(default)]
    pub ok: Option<bool>,
}

/// The card title for an execution: the first non-empty source line, trimmed and
/// length-capped so a long one-liner or a leading comment still reads as a label.
fn exec_title(source: &str) -> String {
    const MAX: usize = 60;
    let line = source
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("python");
    if line.chars().count() > MAX {
        let head: String = line.chars().take(MAX).collect();
        format!("{head}…")
    } else {
        line.to_owned()
    }
}

/// A structured-data body plus the name of the frontend renderer for it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DataView {
    /// Name of a registered frontend renderer. An unknown or empty name falls
    /// back to a generic JSON tree, so a producer can always publish data with
    /// no frontend change.
    #[serde(default)]
    pub renderer: String,
    /// Arbitrary JSON payload handed to the renderer.
    pub data: serde_json::Value,
}

/// One producer process's panes, as sent over its discovery socket.
///
/// `producer` namespaces every pane in `panes` so many processes can share one
/// aggregated document without key collisions. Each message carries the
/// producer's full pane set, so the latest message fully describes that producer
/// and a late-joining reader needs no backlog.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProducerSnapshot {
    /// Stable per-process id: `"<pid>-<short-uuid>"`.
    pub producer: String,
    /// Every pane this producer currently tracks.
    pub panes: Vec<Pane>,
}

/// The directory where producers expose their per-process sockets and the
/// aggregator looks for them.
///
/// Resolved in order: `$IX_DASH_DIR`, then `$XDG_RUNTIME_DIR/ix-dash`, then
/// `/tmp/ix-dash-<user>`. Kept deliberately short: macOS caps a unix socket
/// path (`sun_path`) at 104 bytes, and `$TMPDIR` on macOS is long enough to
/// blow that budget once a filename is appended, so it is not used.
#[must_use]
pub fn discovery_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("IX_DASH_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(runtime).join("ix-dash");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "shared".to_owned());
    PathBuf::from(format!("/tmp/ix-dash-{user}"))
}

/// A unique socket path for the current process inside [`discovery_dir`].
///
/// The filename is `"<pid>-<short-uuid>.sock"`: the pid is human-legible for
/// debugging and the uuid suffix keeps it unique across pid reuse.
#[must_use]
pub fn socket_path() -> PathBuf {
    let short = uuid::Uuid::new_v4().simple().to_string();
    discovery_dir().join(format!("{}-{}.sock", std::process::id(), &short[..8]))
}

#[cfg(test)]
mod tests {
    use super::{ExecView, Pane, ProducerSnapshot, TerminalView, View};

    fn terminal_view(screen: &str) -> TerminalView {
        TerminalView {
            command: "vim".to_owned(),
            args: "-u NONE".to_owned(),
            rows: 24,
            cols: 80,
            alive: true,
            screen: screen.to_owned(),
            cursor_row: 0,
            cursor_col: 0,
            cursor_visible: true,
            cursor_shape: "block".to_owned(),
            exit_code: None,
        }
    }

    /// A terminal view streamed by a producer built before the cursor/exit
    /// fields were added still deserializes: the new fields fall back to their
    /// defaults instead of failing the whole `ProducerSnapshot` parse and
    /// dropping the pane from the dashboard.
    #[test]
    fn old_terminal_wire_shape_deserializes_with_field_defaults() {
        let old = r#"{
            "kind": "terminal", "command": "vim", "args": "-u NONE",
            "rows": 24, "cols": 80, "alive": true, "screen": "hi"
        }"#;
        let view: View = serde_json::from_str(old).expect("old shape parses");
        let View::Terminal(t) = view else {
            panic!("expected terminal view");
        };
        assert_eq!(t.screen, "hi");
        assert_eq!(t.cursor_row, 0);
        assert_eq!(t.cursor_shape, "");
        assert_eq!(t.exit_code, None);
    }

    /// Every view kind round-trips through the internally-tagged JSON wire, so a
    /// heterogeneous producer snapshot survives serialize/deserialize intact.
    #[test]
    fn heterogeneous_snapshot_round_trips() {
        let snapshot = ProducerSnapshot {
            producer: "123-abcd".to_owned(),
            panes: vec![
                Pane::terminal("t1", terminal_view("hello")),
                Pane::html("h1", "notes", "<b>hi</b>"),
                Pane::exec(
                    "e1",
                    ExecView {
                        source: "print('hi')".to_owned(),
                        lang: "python".to_owned(),
                        stdout: "hi\n".to_owned(),
                        stderr: String::new(),
                        result: String::new(),
                        running: false,
                        ok: Some(true),
                    },
                ),
                Pane::data("d1", "metrics", "gauge", serde_json::json!({"cpu": 0.5})),
            ],
        };
        let line = serde_json::to_string(&snapshot).expect("serialize");
        let back: ProducerSnapshot = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(snapshot, back);
        assert_eq!(back.panes[0].view.kind(), "terminal");
        assert_eq!(back.panes[1].view.kind(), "html");
        assert_eq!(back.panes[2].view.kind(), "exec");
        assert_eq!(back.panes[3].view.kind(), "data");
    }

    /// An execution pane titles itself with the first non-empty source line and
    /// preserves its captured output across the wire, including an absent `ok`
    /// while still running.
    #[test]
    fn exec_pane_titles_and_round_trips() {
        let running = Pane::exec(
            "e1",
            ExecView {
                source: "\n  # comment\nsubprocess.run(['echo', 'hi'])\n".to_owned(),
                lang: "python".to_owned(),
                stdout: String::new(),
                stderr: String::new(),
                result: String::new(),
                running: true,
                ok: None,
            },
        );
        assert_eq!(running.title, "# comment");
        let line = serde_json::to_string(&running).expect("serialize");
        let back: Pane = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(running, back);
        let View::Exec(view) = back.view else {
            panic!("expected exec view");
        };
        assert!(view.running);
        assert_eq!(view.ok, None);
    }
}
