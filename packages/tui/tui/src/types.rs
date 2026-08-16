//! Plain value types shared across the crate: terminal colors, styled cells,
//! spawn configuration, and the combined scrollback/viewport read.

/// A VT100 cell color.
///
/// `Default` is the terminal's unset color. `Indexed` is a palette entry
/// (`0..=15` are the ANSI names, `16..=255` the 256-color cube and grayscale
/// ramp). `Rgb` is a 24-bit truecolor triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Color {
    /// The terminal default for this channel.
    #[default]
    Default,
    /// A 256-color palette index.
    Indexed(u8),
    /// A 24-bit truecolor `(r, g, b)` triple.
    Rgb(u8, u8, u8),
}

impl From<ix_vt::StyleColor> for Color {
    fn from(color: ix_vt::StyleColor) -> Self {
        match color {
            ix_vt::StyleColor::None => Self::Default,
            ix_vt::StyleColor::Palette(index) => Self::Indexed(index),
            ix_vt::StyleColor::Rgb(rgb) => Self::Rgb(rgb.r, rgb.g, rgb.b),
        }
    }
}

/// One terminal cell: its character and VT100 styling.
///
/// A cell the terminal never wrote renders as a space with [`Color::Default`]
/// foreground and background; that empty cell is also [`StyledCell::default`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyledCell {
    pub character: char,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl Default for StyledCell {
    fn default() -> Self {
        Self {
            character: ' ',
            fg: Color::Default,
            bg: Color::Default,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

/// The shape the terminal cursor is drawn as.
///
/// Sourced from the VT engine's render state ([`ix_vt::CursorVisualStyle`]),
/// which models the `DECSCUSR` shape natively. The blink distinction is dropped
/// because the dashboard does not animate, and ghostty's unfocused hollow block
/// collapses to a plain block for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// A filled cell. Ghostty's `Block` and `BlockHollow`, the terminal default.
    #[default]
    Block,
    /// A line under the cell.
    Underline,
    /// A vertical bar at the cell's left edge.
    Bar,
}

impl From<ix_vt::CursorVisualStyle> for CursorShape {
    fn from(style: ix_vt::CursorVisualStyle) -> Self {
        match style {
            ix_vt::CursorVisualStyle::Bar => Self::Bar,
            ix_vt::CursorVisualStyle::Underline => Self::Underline,
            ix_vt::CursorVisualStyle::Block | ix_vt::CursorVisualStyle::BlockHollow => Self::Block,
        }
    }
}

impl CursorShape {
    /// A short stable token for the wire and the browser parser.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Underline => "underline",
            Self::Bar => "bar",
        }
    }
}

/// The lifecycle state of a spawned process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitState {
    /// The process is still running.
    Running,
    /// The process has exited. `Some(code)` carries its exit code; `None` means
    /// it was terminated by a signal and so has no exit code.
    Exited(Option<i32>),
}

/// Which agent CLI's session-log family a spawned agent writes, so the
/// producer can resolve and tail the one file behind its transcript pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionLogKind {
    /// `<config dir>/projects/<munged-cwd>/<session-uuid>.jsonl`, written live
    /// by Claude Code. The config dir defaults to `~/.claude`
    /// (`$CLAUDE_CONFIG_DIR` when the child was launched with one; pass it as
    /// [`AgentConfig::log_root`]).
    Claude,
    /// `<home>/sessions/<YYYY>/<MM>/<DD>/rollout-*.jsonl`, written by Codex.
    /// The home defaults to `~/.codex` (`$CODEX_HOME`; pass as
    /// [`AgentConfig::log_root`]).
    Codex,
}

/// How a spawned agent presents on the dashboard: its kind label, the screen
/// markers status inference reads, and where its session log lives.
///
/// The markers mirror the Python harness's grounded values (Claude Code's
/// `"esc to interrupt"` busy footer, the trust-prompt gate fragments); a
/// marker-less agent still gets status from quiescence alone.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentConfig {
    /// Display label: `"claude"`, `"codex"`, a custom harness name.
    pub kind: String,
    /// Substring painted while a turn is in flight (matched case-insensitive):
    /// the precise fast path for "working". `None` relies on quiescence alone.
    pub busy_marker: Option<String>,
    /// Blocking-question fragments (matched case-insensitive). Empty falls
    /// back to the grounded defaults shared with fleetview.
    pub gate_markers: Vec<String>,
    /// The session-log family to tail for the transcript pane, if any.
    pub session_log: Option<SessionLogKind>,
    /// The directory the agent session is keyed by: Claude mungles it into
    /// the project directory name, Codex records it in `session_meta`.
    /// `None` uses this process's working directory at first resolution.
    pub cwd: Option<std::path::PathBuf>,
    /// Root of the agent's config/state tree (`~/.claude`, `~/.codex`),
    /// for a child launched with `CLAUDE_CONFIG_DIR`/`CODEX_HOME` set -- the
    /// resolver must look where the child actually writes. `None` reads the
    /// producer's own environment, then the home default.
    pub log_root: Option<std::path::PathBuf>,
}

/// Spawn-time terminal configuration.
///
/// [`SpawnConfig::default`] is the single source of truth for the defaults:
/// an 80x24 screen with 10,000 lines of scrollback, no extra environment, and
/// no agent presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnConfig {
    /// Terminal height in character rows.
    pub rows: u16,
    /// Terminal width in character columns.
    pub cols: u16,
    /// Lines of history retained above the viewport.
    pub scrollback_lines: usize,
    /// Extra environment for the child, applied in order on top of the
    /// inherited environment. Carries per-session identity/config for spawned
    /// agent harnesses. `TERM`/`COLORTERM` are forced by the crate after these
    /// pairs, so they always win.
    pub env: Vec<(String, String)>,
    /// Agent presentation for the dashboard: kind label, status markers, and
    /// session-log resolution. `None` for a plain terminal.
    pub agent: Option<AgentConfig>,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        Self {
            rows: 24,
            cols: 80,
            scrollback_lines: 10_000,
            env: Vec::new(),
            agent: None,
        }
    }
}

/// The cursor's position and visibility in viewport cell coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPos {
    /// Zero-based row within the viewport.
    pub row: u16,
    /// Zero-based column within the viewport.
    pub col: u16,
    /// Whether the cursor is currently shown.
    pub visible: bool,
}

/// A point-in-time read of a terminal: scrollback history plus the viewport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullOutput {
    /// Lines that have scrolled above the viewport, oldest first.
    pub scrollback: Vec<String>,
    /// The visible screen, top line first.
    pub viewport: Vec<String>,
}
