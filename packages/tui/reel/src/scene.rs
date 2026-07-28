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
    // A short captioned tour run against the real tools. Each `#` line is a shell
    // comment, so it shows as a caption without producing output. Both commands
    // run offline, so anyone can regenerate this clip.
    vec![
        // 1. git-log-pretty: recent commits as colored file-icon trees.
        Action::Type("# recent work, as a colored file-icon tree"),
        Action::Send("\r"),
        Action::Hold(secs(0.7)),
        Action::Type("git-log-pretty --no-pager"),
        Action::Send("\r"),
        Action::Hold(secs(3.6)),
        Action::Type("clear"),
        Action::Send("\r"),
        Action::Hold(secs(0.3)),
        // 2. The PTY driver: this whole clip is code typing into a real shell.
        Action::Type("# drive any terminal program from code"),
        Action::Send("\r"),
        Action::Hold(secs(0.7)),
        Action::Type("python3 -q"),
        Action::Send("\r"),
        Action::WaitFor {
            needle: ">>>",
            max: secs(4.0),
        },
        Action::Type("[tool.upper() for tool in ('search', 'tui', 'clone', 'mcp')]"),
        Action::Send("\r"),
        Action::Hold(secs(2.4)),
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
