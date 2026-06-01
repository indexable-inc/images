//! Encoder for the [kitty terminal graphics protocol].
//!
//! This crate turns image bytes into the `APC _G ... ST` escape sequences that
//! `kitty`, `ghostty`, and `wezterm` understand, and nothing else: it does not
//! open a terminal, decode images, or talk to the network. Callers own those
//! concerns and decide where the returned string is written.
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
    use super::{APC_START, Image, MAX_CHUNK, Placement, ST, STANDARD, place, transmit};
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
