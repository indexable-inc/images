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

impl From<vt100::Color> for Color {
    fn from(color: vt100::Color) -> Self {
        match color {
            vt100::Color::Default => Self::Default,
            vt100::Color::Idx(index) => Self::Indexed(index),
            vt100::Color::Rgb(r, g, b) => Self::Rgb(r, g, b),
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
/// Tracked from the `DECSCUSR` escape (`CSI Ps SP q`) in the byte stream: `0`
/// and `1` are a blinking block, `2` a steady block, `3`/`4` an underline, and
/// `5`/`6` a bar. vt100 itself does not model cursor shape, so the actor sniffs
/// the sequence and the frame builder carries the result to the renderer. The
/// blink distinction is dropped because the dashboard does not animate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorShape {
    /// A filled cell. The terminal default and the `0`/`1`/`2` shapes.
    #[default]
    Block,
    /// A line under the cell (`3`/`4`).
    Underline,
    /// A vertical bar at the cell's left edge (`5`/`6`).
    Bar,
}

impl CursorShape {
    /// Map a `DECSCUSR` parameter to a shape. An unknown value falls back to the
    /// default block, matching how a real terminal treats an unsupported style.
    #[must_use]
    pub const fn from_decscusr(param: u16) -> Self {
        match param {
            3 | 4 => Self::Underline,
            5 | 6 => Self::Bar,
            _ => Self::Block,
        }
    }

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

/// Spawn-time terminal configuration.
///
/// [`SpawnConfig::default`] is the single source of truth for the defaults:
/// an 80x24 screen with 10,000 lines of scrollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnConfig {
    /// Terminal height in character rows.
    pub rows: u16,
    /// Terminal width in character columns.
    pub cols: u16,
    /// Lines of history retained above the viewport.
    pub scrollback_lines: usize,
}

impl Default for SpawnConfig {
    fn default() -> Self {
        Self {
            rows: 24,
            cols: 80,
            scrollback_lines: 10_000,
        }
    }
}

/// A point-in-time read of a terminal: scrollback history plus the viewport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullOutput {
    /// Lines that have scrolled above the viewport, oldest first.
    pub scrollback: Vec<String>,
    /// The visible screen, top line first.
    pub viewport: Vec<String>,
}
