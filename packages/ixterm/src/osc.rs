//! Encoding of the private ix-term OSC (number 5522, from index#3797).
//!
//! Framing is `ESC ] 5522 ; open ; <abs path> BEL`. BEL termination is used
//! because every VT parser accepts it, while 7-bit ST (`ESC \`) support
//! varies.

// Only the linux build reaches this from `main`; unit tests exercise it on
// every platform.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::path::Path;

use anyhow::{Result, bail};

/// Encode an `open` request for `path` as one contiguous buffer, suitable
/// for a single `write(2)`.
///
/// C0 control bytes and DEL terminate or abort an OSC string in VT parsers,
/// so a path containing one cannot be framed; refuse it rather than send a
/// truncated sequence.
pub fn encode_open(path: &Path) -> Result<Vec<u8>> {
    let bytes = path.as_os_str().as_encoded_bytes();
    if bytes.iter().any(|&b| b < 0x20 || b == 0x7f) {
        bail!(
            "path {} contains control bytes and cannot be framed in an OSC sequence",
            path.display(),
        );
    }

    let header: &[u8] = b"\x1b]5522;open;";
    let mut buf = Vec::with_capacity(header.len() + bytes.len() + 1);
    buf.extend_from_slice(header);
    buf.extend_from_slice(bytes);
    buf.push(0x07);
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::encode_open;

    #[test]
    fn encodes_byte_exact() {
        let buf = encode_open(Path::new("/tmp/report.html")).unwrap();
        assert_eq!(buf, b"\x1b]5522;open;/tmp/report.html\x07");
    }

    #[test]
    fn refuses_control_bytes() {
        // BEL would end the sequence early, ESC would abort it, and a
        // newline is plain C0; all three must be refused, not mangled.
        assert!(encode_open(Path::new("/tmp/a\x07b")).is_err());
        assert!(encode_open(Path::new("/tmp/a\x1bb")).is_err());
        assert!(encode_open(Path::new("/tmp/a\nb")).is_err());
    }
}
