//! Legacy chat formatting codes: `§a`, `§l`, and the `&a` config-file
//! spelling.
//!
//! Status MOTDs still carry them, so comparing display text means stripping
//! them first — the same normalization mc-probe applies to both sides of its
//! `--motd-contains` check.

/// Returns `text` with every legacy format code (`§` or `&` followed by a
/// color/style character) removed.
///
/// A `§`/`&` that is *not* followed by a format character is kept: it is
/// content, not markup.
#[must_use]
pub fn strip_format_codes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if (c == '\u{a7}' || c == '&') && chars.peek().copied().is_some_and(is_format_code) {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// The character class vanilla accepts after `§`: colors `0-9a-f`, styles
/// `k-o`, reset `r`, and the modern hex-color escape `x` — either case.
const fn is_format_code(c: char) -> bool {
    c.is_ascii_hexdigit() || matches!(c.to_ascii_lowercase(), 'k'..='o' | 'r' | 'x')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_section_sign_codes() {
        assert_eq!(strip_format_codes("\u{a7}aHello \u{a7}lWorld"), "Hello World");
    }

    #[test]
    fn strips_ampersand_codes() {
        assert_eq!(strip_format_codes("&6Spleef&r arena"), "Spleef arena");
    }

    #[test]
    fn keeps_literal_ampersand() {
        assert_eq!(strip_format_codes("Fish & Chips"), "Fish & Chips");
    }

    #[test]
    fn keeps_trailing_marker() {
        assert_eq!(strip_format_codes("dangling \u{a7}"), "dangling \u{a7}");
    }

    #[test]
    fn strips_uppercase_and_hex_escape() {
        assert_eq!(strip_format_codes("\u{a7}X\u{a7}A\u{a7}Rplain"), "plain");
    }
}
