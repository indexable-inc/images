//! ANSI styling. Callers print through `anstream` so pipes stay clean.

use anstyle::{AnsiColor, Style};

pub const CYAN: Style = AnsiColor::Cyan.on_default();
pub const GREEN: Style = AnsiColor::Green.on_default();
pub const YELLOW: Style = AnsiColor::Yellow.on_default();
pub const RED: Style = AnsiColor::Red.on_default();

/// Wrap `text` in the style's escape sequences.
#[must_use]
pub fn paint(style: Style, text: &str) -> String {
    format!("{}{text}{}", style.render(), style.render_reset())
}
