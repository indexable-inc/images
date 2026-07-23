//! Detect whether the terminal background is light or dark.
//!
//! This owns one decision shared across the repo's terminal tools: probe the
//! terminal for its background luma via [`terminal_light`] and classify it as
//! [`Theme::Light`] or [`Theme::Dark`]. The probe writes an OSC color query and
//! waits for the terminal to answer, so it only runs when stdout is a TTY.
//! Under a pipe, a capture, or a test the query bytes would corrupt the output
//! (or block on a reply that never comes), so non-interactive stdout falls back
//! to [`Theme::Dark`], the common terminal default.
//!
//! Callers with their own "should I emit color at all" flag (for example a
//! `--color` switch) keep that decision local and only call [`detect`] once
//! they've decided color is wanted.

use std::io::IsTerminal;

/// Whether the terminal background reads as light or dark. Tools map this to
/// their own palettes; the shared part is only the classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    /// A light background, or anything brighter than mid-gray.
    Light,
    /// A dark background, and the fallback when the terminal cannot be probed.
    #[default]
    Dark,
}

/// Probe the terminal background and classify it.
///
/// Returns [`Theme::Dark`] without probing when stdout is not a terminal, so
/// the query bytes never leak into a pipe or capture. Anything brighter than
/// mid-gray (luma above `0.5`) counts as light; an unreadable or absent
/// response also falls back to [`Theme::Dark`].
#[must_use]
pub fn detect() -> Theme {
    if !std::io::stdout().is_terminal() {
        return Theme::Dark;
    }
    match terminal_light::luma() {
        Ok(luma) if luma > 0.5 => Theme::Light,
        _ => Theme::Dark,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_dark() {
        assert_eq!(Theme::default(), Theme::Dark);
    }

    #[test]
    fn non_tty_stdout_falls_back_to_dark() {
        // The test harness captures stdout, so it is never a TTY here; detect
        // must not probe and must return the dark fallback.
        assert_eq!(detect(), Theme::Dark);
    }
}
