//! Literal spellings shared by the host emitters.

/// `text` as a double-quoted string literal.
///
/// Python and TypeScript spell this escape set identically -- the backslash,
/// the quote, and the three whitespace escapes -- and pass every other
/// character through, so both emitters render a literal from here instead of
/// each keeping the same table. A target whose escaping actually differs
/// (Java's `\uXXXX` control chars, say) spells its own.
pub fn double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
