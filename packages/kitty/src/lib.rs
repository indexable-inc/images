//! Encoder for the [kitty terminal graphics protocol].
//!
//! This crate turns image bytes into the `APC _G ... ST` escape sequences that
//! `kitty`, `ghostty`, and `wezterm` understand, and nothing else: it does not
//! open a terminal, decode images, or talk to the network. Callers own those
//! concerns and decide where the returned string is written.
//!
//! Two display models are supported. [`transmit`] and [`place`] draw an image at
//! the cursor, which is simplest but pins the image to a screen position, so it
//! cannot survive a program that repaints the screen (a pager, `tmux`, an
//! editor). [`transmit_virtual`] plus [`placeholder_row`] use the protocol's
//! [Unicode placeholder] feature instead: the pixels are transmitted once, then
//! the image is displayed wherever ordinary `U+10EEEE` text cells are printed.
//! Because those cells are normal text, a host that knows nothing about graphics
//! moves the image around as it reflows the text, so the image scrolls with the
//! output and pages cleanly.
//!
//! [Unicode placeholder]: https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders
//!
//! ```
//! let png: &[u8] = b"\x89PNG..."; // real `PNG` bytes in practice
//! let seq = kitty::transmit(
//!     &kitty::Image::Png(png),
//!     None,
//!     &kitty::Placement { rows: Some(2), cols: Some(4), move_cursor: false },
//! );
//! assert!(seq.starts_with("\x1b_G"));
//! ```
//!
//! [kitty terminal graphics protocol]: https://sw.kovidgoyal.net/kitty/graphics-protocol/

use std::fmt::Write;
use std::sync::OnceLock;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

/// Start of an Application Programming Command carrying a graphics command.
const APC_START: &str = "\x1b_G";
/// String Terminator (`ESC \`) that closes each command.
const ST: &str = "\x1b\\";
/// The protocol requires the base64 payload be split into chunks no larger than
/// this many bytes, each sent as its own command.
const MAX_CHUNK: usize = 4096;

/// Image data to transmit to the terminal.
#[derive(Debug, Clone, Copy)]
pub enum Image<'a> {
    /// `PNG` file bytes. The terminal decodes them, so no dimensions are needed
    /// (protocol format `f=100`).
    Png(&'a [u8]),
    /// Raw 8-bit `RGBA` pixels, exactly `width * height * 4` bytes, row-major
    /// (protocol format `f=32`).
    Rgba {
        width: u32,
        height: u32,
        pixels: &'a [u8],
    },
}

/// How the image occupies the terminal cell grid when displayed.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    /// Columns to scale the image across (`c=`). `None` lets the terminal pick.
    pub cols: Option<u32>,
    /// Rows to scale the image across (`r=`). `None` lets the terminal pick.
    pub rows: Option<u32>,
    /// Whether the cursor advances past the image after display.
    ///
    /// When `false`, the command sets `C=1` (do not move cursor) so the caller
    /// can position the cursor and lay out text around the image itself.
    pub move_cursor: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            cols: None,
            rows: None,
            move_cursor: true,
        }
    }
}

/// Best-effort detection of whether the current terminal speaks the protocol.
///
/// This reads environment variables only; it never queries the terminal. A
/// `true` result is a strong hint, not a guarantee, so callers should still
/// offer an opt-out.
#[must_use]
pub fn is_supported() -> bool {
    // Terminal multiplexers usually swallow graphics escapes (they render as
    // garbage), so treat tmux/screen sessions as unsupported by default.
    if std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some() {
        return false;
    }
    if std::env::var_os("KITTY_WINDOW_ID").is_some() {
        return true;
    }
    let advertises = |value: &str| {
        let value = value.to_ascii_lowercase();
        value.contains("kitty") || value.contains("ghostty") || value.contains("wezterm")
    };
    std::env::var("TERM").is_ok_and(|term| advertises(&term))
        || std::env::var("TERM_PROGRAM").is_ok_and(|program| advertises(&program))
}

/// Transmit `image` and display it at the cursor.
///
/// When `id` is `Some`, the terminal stores the image under that id so the same
/// pixels can be redrawn later with [`place`] instead of resending them.
#[must_use]
pub fn transmit(image: &Image<'_>, id: Option<u32>, placement: &Placement) -> String {
    let (format, payload, dimensions): (u16, &[u8], Option<(u32, u32)>) = match *image {
        Image::Png(bytes) => (100, bytes, None),
        Image::Rgba {
            width,
            height,
            pixels,
        } => (32, pixels, Some((width, height))),
    };

    let mut control = format!("a=T,f={format},q=2");
    if let Some((width, height)) = dimensions {
        let _ = write!(control, ",s={width},v={height}");
    }
    if let Some(id) = id {
        let _ = write!(control, ",i={id}");
    }
    push_placement(&mut control, placement);

    encode_chunks(&control, payload)
}

/// Display an image previously sent by [`transmit`] with the same `id`, at the
/// cursor. Sends no pixels, so it is cheap to repeat.
#[must_use]
pub fn place(id: u32, placement: &Placement) -> String {
    let mut control = format!("a=p,i={id},q=2");
    push_placement(&mut control, placement);
    format!("{APC_START}{control}{ST}")
}

/// The Unicode placeholder character (`U+10EEEE`).
///
/// Printing this cell, with the image id in its foreground color and row/column
/// diacritics, displays a fragment of a virtual image previously sent by
/// [`transmit_virtual`].
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// Transmit `image` and create an invisible *virtual placement* for it.
///
/// The placement is sized to a `cols`×`rows` cell box. Nothing is drawn until
/// matching [`placeholder_row`] cells are printed; the image is then fit to the
/// box with its aspect ratio kept.
///
/// `id` must be non-zero and fit in 24 bits, because [`placeholder_row`] encodes
/// it in a cell's 24-bit foreground color. Quiet mode (`q=2`) is set so the
/// terminal sends no acknowledgement that could corrupt a host program's output.
#[must_use]
pub fn transmit_virtual(image: &Image<'_>, id: u32, cols: u32, rows: u32) -> String {
    let (format, payload, dimensions): (u16, &[u8], Option<(u32, u32)>) = match *image {
        Image::Png(bytes) => (100, bytes, None),
        Image::Rgba {
            width,
            height,
            pixels,
        } => (32, pixels, Some((width, height))),
    };

    // a=T transmits and (with U=1) creates a virtual placement in one command.
    let mut control = format!("a=T,U=1,f={format},i={id},c={cols},r={rows},q=2");
    if let Some((width, height)) = dimensions {
        let _ = write!(control, ",s={width},v={height}");
    }
    encode_chunks(&control, payload)
}

/// Render row `row` of a Unicode-placeholder image as ordinary text.
///
/// Emits a foreground-color escape carrying the image `id`, then `cols`
/// placeholder cells tagged with the row and column diacritics, then a
/// foreground reset so following text is unstyled.
///
/// Callers lay these rows out as ordinary text (one per terminal line, in the
/// gutter beside other content). `id` must match a prior [`transmit_virtual`]
/// and fit in 24 bits. `row` and `col` indices beyond the diacritic table
/// (hundreds of entries) are dropped, which only clips an unreasonably tall or
/// wide placement.
#[must_use]
pub fn placeholder_row(id: u32, row: u32, cols: u32) -> String {
    let table = diacritics();
    let Some(row_mark) = table.get(row as usize) else {
        return String::new();
    };

    // The image id rides in the 24-bit true-color foreground; the terminal reads
    // it back from the cell color rather than displaying the color itself.
    let (r, g, b) = ((id >> 16) & 0xFF, (id >> 8) & 0xFF, id & 0xFF);
    let mut out = format!("\x1b[38;2;{r};{g};{b}m");
    for col in 0..cols {
        let Some(col_mark) = table.get(col as usize) else {
            break;
        };
        out.push(PLACEHOLDER);
        out.push(*row_mark);
        out.push(*col_mark);
    }
    // Reset only the foreground (SGR 39) so the caller's own styling is intact.
    out.push_str("\x1b[39m");
    out
}

/// The row/column diacritics, parsed once from the vendored kitty table.
///
/// Index `i` is the combining character that encodes the number `i`.
fn diacritics() -> &'static [char] {
    static TABLE: OnceLock<Vec<char>> = OnceLock::new();
    TABLE.get_or_init(|| {
        // Source: kitty `gen/rowcolumn-diacritics.txt`. Each data line is
        // `HEX;NAME;...`; comment lines start with `#`. We take the leading hex.
        include_str!("rowcolumn-diacritics.txt")
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
            .filter_map(|line| line.split(';').next())
            .filter_map(|hex| u32::from_str_radix(hex.trim(), 16).ok())
            .filter_map(char::from_u32)
            .collect()
    })
}

fn push_placement(control: &mut String, placement: &Placement) {
    if let Some(cols) = placement.cols {
        let _ = write!(control, ",c={cols}");
    }
    if let Some(rows) = placement.rows {
        let _ = write!(control, ",r={rows}");
    }
    if !placement.move_cursor {
        // C=1 tells the terminal to leave the cursor where it was.
        control.push_str(",C=1");
    }
}

/// Base64-encode `payload` and frame it as one or more graphics commands,
/// splitting into `m=1` continuation chunks when it exceeds [`MAX_CHUNK`].
fn encode_chunks(control: &str, payload: &[u8]) -> String {
    let encoded = STANDARD.encode(payload);

    if encoded.len() <= MAX_CHUNK {
        return format!("{APC_START}{control},m=0;{encoded}{ST}");
    }

    let total = encoded.len();
    let mut out = String::with_capacity(total + (total / MAX_CHUNK + 1) * (APC_START.len() + 16));
    let mut start = 0;
    let mut first = true;
    while start < total {
        let end = (start + MAX_CHUNK).min(total);
        // `encoded` is base64, i.e. pure ASCII, so every byte offset is also a
        // valid char boundary and this slice never splits a code point.
        let chunk = &encoded[start..end];
        let last = end == total;

        out.push_str(APC_START);
        if first {
            // The first chunk carries the real control keys; the rest carry `m`.
            out.push_str(control);
            out.push_str(",m=1;");
            first = false;
        } else if last {
            out.push_str("m=0;");
        } else {
            out.push_str("m=1;");
        }
        out.push_str(chunk);
        out.push_str(ST);

        start = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        APC_START, Image, MAX_CHUNK, PLACEHOLDER, Placement, ST, STANDARD, diacritics, place,
        placeholder_row, transmit, transmit_virtual,
    };
    use base64::Engine;

    fn single_command(sequence: &str) -> (&str, &str) {
        let body = sequence
            .strip_prefix(APC_START)
            .and_then(|rest| rest.strip_suffix(ST))
            .expect("framed as one APC command");
        body.split_once(';').expect("control;payload")
    }

    #[test]
    fn png_single_chunk_has_control_and_payload() {
        let sequence = transmit(&Image::Png(b"hello"), None, &Placement::default());
        let (control, payload) = single_command(&sequence);
        assert!(control.contains("a=T"));
        assert!(control.contains("f=100"));
        assert!(control.contains("q=2"));
        assert!(control.contains("m=0"));
        assert_eq!(payload, STANDARD.encode(b"hello"));
    }

    #[test]
    fn rgba_carries_dimensions_and_placement() {
        let pixels = [0u8; 16]; // 2x2 RGBA
        let sequence = transmit(
            &Image::Rgba {
                width: 2,
                height: 2,
                pixels: &pixels,
            },
            Some(7),
            &Placement {
                cols: Some(4),
                rows: Some(2),
                move_cursor: false,
            },
        );
        let (control, _) = single_command(&sequence);
        for key in ["f=32", "s=2", "v=2", "i=7", "c=4", "r=2", "C=1"] {
            assert!(control.contains(key), "missing {key} in {control}");
        }
    }

    #[test]
    fn large_payload_is_chunked() {
        // 9 KiB raw -> ~12 KiB base64 -> 3 chunks of <= 4096.
        let big = vec![0xABu8; 9 * 1024];
        let sequence = transmit(&Image::Png(&big), None, &Placement::default());
        let commands: Vec<&str> = sequence.split(ST).filter(|part| !part.is_empty()).collect();
        assert!(commands.len() >= 3, "expected multiple chunks");
        assert!(commands[0].contains("a=T"));
        assert!(commands[0].contains("m=1"));
        let last = commands.last().expect("at least one command");
        assert!(last.contains("m=0"));
        assert!(!last.contains("a=T"));
        for command in &commands {
            let payload = command.rsplit_once(';').map_or("", |(_, payload)| payload);
            assert!(payload.len() <= MAX_CHUNK);
        }
    }

    #[test]
    fn virtual_transmit_marks_unicode_placement() {
        let sequence = transmit_virtual(&Image::Png(b"hello"), 42, 4, 2);
        let (control, payload) = single_command(&sequence);
        // U=1 makes the placement virtual; q=2 silences acknowledgements.
        for key in ["a=T", "U=1", "f=100", "i=42", "c=4", "r=2", "q=2", "m=0"] {
            assert!(control.contains(key), "missing {key} in {control}");
        }
        assert_eq!(payload, STANDARD.encode(b"hello"));
    }

    #[test]
    fn placeholder_row_encodes_id_in_foreground() {
        // id 42 = 0x00002A -> fg color 0;0;42.
        let row = placeholder_row(42, 0, 4);
        assert!(row.starts_with("\x1b[38;2;0;0;42m"), "fg color must carry the id: {row:?}");
        assert!(row.ends_with("\x1b[39m"), "must reset the foreground");
        assert_eq!(row.matches(PLACEHOLDER).count(), 4, "one placeholder cell per column");
    }

    #[test]
    fn placeholder_row_uses_distinct_row_and_col_diacritics() {
        let table = diacritics();
        let (row0, col0, col1) = (table[0], table[0], table[1]);
        // Row 0, two columns: each cell is placeholder + row-mark + col-mark.
        let row = placeholder_row(7, 0, 2);
        let expected = format!("{PLACEHOLDER}{row0}{col0}{PLACEHOLDER}{row0}{col1}");
        assert!(row.contains(&expected), "cells must tag row 0, cols 0 and 1: {row:?}");
    }

    #[test]
    fn diacritics_table_matches_kitty_spec() {
        let table = diacritics();
        // The first two entries are the canonical examples from the kitty docs.
        assert_eq!(table[0], '\u{0305}', "index 0 is COMBINING OVERLINE");
        assert_eq!(table[1], '\u{030D}', "index 1 is COMBINING VERTICAL LINE ABOVE");
        assert!(table.len() > 255, "need enough diacritics to index any row/column");
    }

    #[test]
    fn place_references_id_without_payload() {
        let sequence = place(
            7,
            &Placement {
                cols: Some(4),
                rows: Some(2),
                move_cursor: false,
            },
        );
        assert!(sequence.starts_with(APC_START));
        assert!(sequence.ends_with(ST));
        assert!(sequence.contains("a=p"));
        assert!(sequence.contains("i=7"));
        assert!(!sequence.contains(';'), "a placement of an existing image sends no payload");
    }
}
