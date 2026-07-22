//! The single error type: every failure (parse error or missing 1:1 mapping)
//! carries a source position and renders like a compiler diagnostic.

use std::fmt;

/// A positioned conversion error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    message: String,
    /// 1-based line of the offending source.
    line: usize,
    /// 1-based column (in characters) within that line.
    column: usize,
    /// The full source line, for the diagnostic snippet.
    line_text: String,
}

impl Error {
    /// Builds an error pointing at byte `offset` of `source`.
    pub(crate) fn at(offset: usize, source: &str, message: impl Into<String>) -> Self {
        let offset = offset.min(source.len());

        let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
        let line_end = source[offset..]
            .find('\n')
            .map_or(source.len(), |i| offset + i);
        let line = source[..line_start].matches('\n').count() + 1;
        let column = source[line_start..offset].chars().count() + 1;

        Self {
            message: message.into(),
            line,
            column,
            line_text: source[line_start..line_end].to_owned(),
        }
    }

    /// [`Self::at`] for the `u32` byte offsets oxc spans and diagnostics carry.
    pub(crate) fn at_offset32(offset: u32, source: &str, message: impl Into<String>) -> Self {
        let offset = usize::try_from(offset).expect("u32 offset fits usize");
        Self::at(offset, source, message)
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// 1-based line number.
    #[must_use]
    pub const fn line(&self) -> usize {
        self.line
    }

    /// 1-based column number, counted in characters.
    #[must_use]
    pub const fn column(&self) -> usize {
        self.column
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            message,
            line,
            column,
            line_text,
        } = self;
        let gutter = line.to_string().len();

        writeln!(f, "error: {message}")?;
        writeln!(f, "{:gutter$} --> {line}:{column}", "")?;
        writeln!(f, "{:gutter$} |", "")?;
        writeln!(f, "{line} | {line_text}")?;
        write!(f, "{:gutter$} | {:>column$}", "", "^")
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_map_to_line_and_column() {
        let source = "ab\ncdef\ng";
        let error = Error::at(5, source, "boom");
        assert_eq!((error.line(), error.column()), (2, 3));
        assert_eq!(error.message(), "boom");
    }

    #[test]
    fn offset_past_the_end_clamps_to_the_last_position() {
        let error = Error::at(999, "ab\ncd", "boom");
        assert_eq!((error.line(), error.column()), (2, 3));
    }

    #[test]
    fn column_counts_characters_not_bytes() {
        // "é" is two bytes; the caret must sit on the character grid.
        let source = "éé x";
        let error = Error::at(4, source, "boom");
        assert_eq!((error.line(), error.column()), (1, 3));
    }

    #[test]
    fn display_is_a_compiler_style_diagnostic() {
        let error = Error::at(4, "a\nb === c\n", "no strict equality");
        let expected = "error: no strict equality\n  --> 2:3\n  |\n2 | b === c\n  |   ^";
        assert_eq!(error.to_string(), expected);
    }
}
