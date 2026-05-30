use uuid::Uuid;

use crate::types::StyledCell;
use crate::{Error, error::Result};

const SCROLLBACK_OFFSET_MAX: usize = usize::MAX;

/// The viewport's non-blank lines, top first. An empty screen is a valid state
/// and yields an empty `Vec`; callers that need to wait for first paint use
/// [`super::super::manager`]'s `read_blocking`, which polls on emptiness. This
/// keeps `snapshot`-style reads total instead of erroring on a blank screen.
pub fn extract_viewport_lines(parser: &vt100::Parser) -> Vec<String> {
    parser
        .screen()
        .contents()
        .lines()
        .map(std::string::ToString::to_string)
        .collect()
}

/// The cursor's `(row, col, visible)` as the screen reports it.
///
/// `visible` is the inverse of vt100's `hide_cursor`: a screen that hid its
/// cursor (`CSI ?25l`) should not draw one in the dashboard. Position is in
/// viewport cell coordinates, the same basis as [`extract_styled_cells`].
pub fn extract_cursor(parser: &vt100::Parser) -> (u16, u16, bool) {
    let screen = parser.screen();
    let (row, col) = screen.cursor_position();
    (row, col, !screen.hide_cursor())
}

pub fn extract_scrollback_lines(parser: &mut vt100::Parser) -> Vec<String> {
    parser.screen_mut().set_scrollback(SCROLLBACK_OFFSET_MAX);
    let total_scrollback_lines = parser.screen().scrollback();

    if total_scrollback_lines == 0 {
        parser.screen_mut().set_scrollback(0);
        return Vec::new();
    }

    let mut all_lines = Vec::with_capacity(total_scrollback_lines);

    for offset in (1..=total_scrollback_lines).rev() {
        parser.screen_mut().set_scrollback(offset);
        let contents = parser.screen().contents();

        if let Some(first_line) = contents.lines().next() {
            all_lines.push(first_line.to_string());
        }
    }

    parser.screen_mut().set_scrollback(0);

    all_lines
}

pub fn extract_chars(id: Uuid, parser: &vt100::Parser) -> Result<Vec<Vec<char>>> {
    let screen = parser.screen();
    let (rows, cols) = screen.size();

    if rows == 0 || cols == 0 {
        return Err(Error::NoOutputAvailable { id });
    }

    let mut result = Vec::with_capacity(usize::from(rows));

    for row in 0..rows {
        let mut row_chars = Vec::with_capacity(usize::from(cols));
        for col in 0..cols {
            let ch = match screen.cell(row, col) {
                Some(cell) => cell.contents().chars().next().unwrap_or(' '),
                None => ' ',
            };
            row_chars.push(ch);
        }
        result.push(row_chars);
    }

    Ok(result)
}

fn cell_at(screen: &vt100::Screen, row: u16, col: u16) -> StyledCell {
    match screen.cell(row, col) {
        Some(cell) => StyledCell {
            character: cell.contents().chars().next().unwrap_or(' '),
            fg: cell.fgcolor().into(),
            bg: cell.bgcolor().into(),
            bold: cell.bold(),
            italic: cell.italic(),
            underline: cell.underline(),
            inverse: cell.inverse(),
        },
        None => StyledCell::default(),
    }
}

pub fn extract_styled_cells(
    id: Uuid,
    parser: &vt100::Parser,
) -> Result<ndarray::Array2<StyledCell>> {
    let screen = parser.screen();
    let (rows, cols) = screen.size();

    if rows == 0 || cols == 0 {
        return Err(Error::NoOutputAvailable { id });
    }

    let rows_usize = usize::from(rows);
    let cols_usize = usize::from(cols);

    let mut data = Vec::with_capacity(rows_usize * cols_usize);
    for row in 0..rows {
        for col in 0..cols {
            data.push(cell_at(screen, row, col));
        }
    }

    ndarray::Array2::from_shape_vec((rows_usize, cols_usize), data).map_err(|source| {
        Error::ArrayConversion {
            rows: rows_usize,
            cols: cols_usize,
            source,
        }
    })
}
