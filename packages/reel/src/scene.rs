//! The recorded frame types and the demo script.
//!
//! A [`Frame`] is either a captured terminal screen or a rendered title/outro
//! card. The [`script`] is the sequence of [`Action`]s the recorder types into a
//! real shell; [`title_card`] and [`outro_card`] bookend the recording.

use ndarray::Array2;
use tui::StyledCell;

/// The shell prompt the recorder sets and the outro card references.
pub const PROMPT: &str = "~/index ❯ ";

/// The cursor position and visibility for a captured terminal frame.
#[derive(Clone, Copy, Debug)]
pub struct Cursor {
    pub row: u16,
    pub col: u16,
    pub visible: bool,
}

/// A centered card: a large title, an optional subtitle, an optional footer.
#[derive(Clone, Debug)]
pub struct Card {
    pub title: String,
    pub subtitle: Option<String>,
    pub footer: Option<String>,
}

/// One frame of the reel.
#[derive(Clone)]
pub enum Frame {
    /// A captured terminal screen: the styled grid plus the cursor.
    Terminal {
        cells: Array2<StyledCell>,
        cursor: Cursor,
    },
    /// A rendered card.
    Card(Card),
}

/// One scripted action in the recorded shell session.
pub enum Action {
    /// Type text one character at a time (one captured frame per character).
    Type(&'static str),
    /// Send raw bytes (such as a carriage return) and capture one frame.
    Send(&'static str),
    /// Hold the current screen for this many frames.
    Hold(u32),
    /// Capture frames until the viewport contains `needle`, up to `max` frames.
    WaitFor { needle: &'static str, max: u32 },
}

/// The demo: a real git history view, then a live Python REPL driven through the
/// PTY. `fps` scales the hold durations so the pacing is the same at any frame
/// rate.
#[must_use]
pub fn script(fps: u32) -> Vec<Action> {
    let secs = |n: f32| -> u32 { (n * fps as f32) as u32 };
    vec![
        // One repo, real history.
        Action::Type("git --no-pager -c color.ui=always log --graph --oneline -10"),
        Action::Send("\r"),
        Action::Hold(secs(2.4)),
        // Clear, then drive a real interactive program: a live Python REPL.
        Action::Type("clear"),
        Action::Send("\r"),
        Action::Hold(secs(0.4)),
        Action::Type("python3 -q"),
        Action::Send("\r"),
        Action::WaitFor {
            needle: ">>>",
            max: secs(4.0),
        },
        Action::Type("import sys; sys.version.split()[0]"),
        Action::Send("\r"),
        Action::Hold(secs(1.2)),
        Action::Type("sum(range(10_000_000))"),
        Action::Send("\r"),
        Action::Hold(secs(1.4)),
        Action::Type("[tool.upper() for tool in ('search', 'tui', 'mcp')]"),
        Action::Send("\r"),
        Action::Hold(secs(2.2)),
        // Leave the REPL cleanly so the last frame is a calm prompt.
        Action::Send("\x04"),
        Action::Hold(secs(0.8)),
    ]
}

/// The opening card.
#[must_use]
pub fn title_card() -> Card {
    Card {
        title: "index".to_owned(),
        subtitle: Some("a shared monorepo of dev tools".to_owned()),
        footer: Some("filmed with our own PTY driver".to_owned()),
    }
}

/// The closing card.
#[must_use]
pub fn outro_card() -> Card {
    Card {
        title: "one repo, shared tools".to_owned(),
        subtitle: Some("semantic search · PTY driver · agent loops · MCP".to_owned()),
        footer: Some("github.com/indexable-inc/index".to_owned()),
    }
}
