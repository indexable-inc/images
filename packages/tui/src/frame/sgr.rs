//! Encode a viewport of styled cells into minimal ANSI SGR runs.
//!
//! [`TerminalFrame::screen`](super::TerminalFrame) stays a `String` on the
//! wire, but now carries SGR escapes so the dashboard can paint color, weight,
//! and inverse video. Rust owns this encoding so every reader (the producer's
//! JSON and the in-process Loro doc) renders identical bytes; the browser only
//! parses.
//!
//! The encoder is minimal: it emits an SGR escape only when the active style
//! changes from cell to cell, and a single reset (`CSI 0 m`) at the end of any
//! row that left a non-default style active. Trailing default cells on a row
//! are trimmed so a blank tail does not bloat the payload.

use crate::types::{Color, StyledCell};

/// SGR control sequence introducer.
const CSI: &str = "\x1b[";
/// Full reset of every SGR attribute.
const RESET: &str = "\x1b[0m";

/// Encode `cells` (`[row][col]`) into a newline-joined string with SGR runs.
///
/// Each inner slice is one row. A row is rendered left to right; the active
/// style is reset to default at the start of each row, so a row never inherits
/// the previous row's color.
pub fn encode(cells: &[Vec<StyledCell>]) -> String {
    let mut out = String::new();
    for (index, row) in cells.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        encode_row(&mut out, row);
    }
    out
}

/// Append one row's encoding, tracking the active style so an escape is emitted
/// only on a change, and closing with a reset when the row ends mid-style.
fn encode_row(out: &mut String, row: &[StyledCell]) {
    let trimmed = trim_default_tail(row);
    let default_style = cell_style(&StyledCell::default());
    let mut active = default_style;

    for cell in trimmed {
        let style = cell_style(cell);
        if style != active {
            push_sgr(out, cell);
            active = style;
        }
        out.push(cell.character);
    }

    // Reset only when the row ends with a non-default style still active, so a
    // plain trailing cell does not emit a redundant reset.
    if active != default_style {
        out.push_str(RESET);
    }
}

/// A row's cells up to its last non-default-styled, non-space cell.
///
/// A blank tail (default style, space character) carries no visible information,
/// so dropping it keeps the wire small without changing what the browser draws.
fn trim_default_tail(row: &[StyledCell]) -> &[StyledCell] {
    let default = StyledCell::default();
    let end = row
        .iter()
        .rposition(|cell| cell != &default)
        .map_or(0, |last| last + 1);
    #[allow(clippy::indexing_slicing, reason = "end is a valid rposition + 1, so <= len")]
    &row[..end]
}

/// The style-only view of a cell, used to decide when a new escape is needed.
const fn cell_style(cell: &StyledCell) -> (Color, Color, bool, bool, bool, bool) {
    (
        cell.fg,
        cell.bg,
        cell.bold,
        cell.italic,
        cell.underline,
        cell.inverse,
    )
}

/// Push a full SGR escape that sets exactly `cell`'s style. Always emits a reset
/// first, so the sequence is self-contained and does not depend on the prior
/// cell's attributes.
fn push_sgr(out: &mut String, cell: &StyledCell) {
    let mut params: Vec<String> = vec!["0".to_owned()];
    if cell.bold {
        params.push("1".to_owned());
    }
    if cell.italic {
        params.push("3".to_owned());
    }
    if cell.underline {
        params.push("4".to_owned());
    }
    if cell.inverse {
        params.push("7".to_owned());
    }
    push_color(&mut params, cell.fg, ColorRole::Foreground);
    push_color(&mut params, cell.bg, ColorRole::Background);

    out.push_str(CSI);
    out.push_str(&params.join(";"));
    out.push('m');
}

/// Which channel a color sets, picking the SGR base codes.
#[derive(Clone, Copy)]
enum ColorRole {
    Foreground,
    Background,
}

impl ColorRole {
    /// The base SGR code for the named 16-color ANSI palette: `30`/`40` for the
    /// first eight, `90`/`100` for the bright eight.
    const fn named_base(self, bright: bool) -> u16 {
        match (self, bright) {
            (Self::Foreground, false) => 30,
            (Self::Foreground, true) => 90,
            (Self::Background, false) => 40,
            (Self::Background, true) => 100,
        }
    }

    /// The extended-color selector (`38` foreground, `48` background) used for
    /// 256-palette and truecolor.
    const fn extended(self) -> &'static str {
        match self {
            Self::Foreground => "38",
            Self::Background => "48",
        }
    }
}

/// Append the SGR parameters for one color in one channel. A default color emits
/// nothing because the leading reset already cleared the channel.
fn push_color(params: &mut Vec<String>, color: Color, role: ColorRole) {
    match color {
        Color::Default => {}
        Color::Indexed(index) if index < 8 => {
            params.push((role.named_base(false) + u16::from(index)).to_string());
        }
        Color::Indexed(index) if index < 16 => {
            params.push((role.named_base(true) + u16::from(index - 8)).to_string());
        }
        Color::Indexed(index) => {
            params.push(role.extended().to_owned());
            params.push("5".to_owned());
            params.push(index.to_string());
        }
        Color::Rgb(r, g, b) => {
            params.push(role.extended().to_owned());
            params.push("2".to_owned());
            params.push(r.to_string());
            params.push(g.to_string());
            params.push(b.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(ch: char) -> StyledCell {
        StyledCell {
            character: ch,
            ..StyledCell::default()
        }
    }

    /// A row with no styling and a blank tail encodes to just its visible text.
    #[test]
    fn unstyled_row_is_plain_text() {
        let row = vec![plain('h'), plain('i'), plain(' '), plain(' ')];
        assert_eq!(encode(&[row]), "hi");
    }

    /// A colored cell emits an opening SGR and a closing reset; the run only
    /// re-emits when the style actually changes.
    #[test]
    fn colored_run_emits_one_escape_then_resets() {
        let red = StyledCell {
            character: 'X',
            fg: Color::Indexed(1),
            bold: true,
            ..StyledCell::default()
        };
        let row = vec![red.clone(), red, plain('y')];
        // bold + red fg, two X's under one escape, reset before the plain 'y'.
        assert_eq!(encode(&[row]), "\x1b[0;1;31mXX\x1b[0my");
    }

    /// Truecolor and 256-palette use the extended selector form.
    #[test]
    fn extended_colors_use_38_48_form() {
        let cell = StyledCell {
            character: 'p',
            fg: Color::Rgb(10, 20, 30),
            bg: Color::Indexed(200),
            ..StyledCell::default()
        };
        assert_eq!(encode(&[vec![cell]]), "\x1b[0;38;2;10;20;30;48;5;200mp\x1b[0m");
    }

    /// Each row resets its own style; a colored last cell on row 0 does not bleed
    /// into row 1.
    #[test]
    fn rows_do_not_inherit_style() {
        let red = StyledCell {
            character: 'a',
            fg: Color::Indexed(1),
            ..StyledCell::default()
        };
        assert_eq!(encode(&[vec![red], vec![plain('b')]]), "\x1b[0;31ma\x1b[0m\nb");
    }
}
