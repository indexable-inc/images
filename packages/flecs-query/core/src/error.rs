//! Parse errors with byte spans into the original expression.

use std::fmt;

/// A byte range into the source expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Span {
    /// First byte of the offending region.
    pub start: usize,
    /// One past the last byte of the offending region.
    pub end: usize,
}

impl Span {
    /// A single-position span.
    #[must_use]
    pub const fn at(offset: usize) -> Self {
        Self {
            start: offset,
            end: offset,
        }
    }
}

/// A syntax error in a query expression.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParseError {
    /// What went wrong.
    pub message: String,
    /// Where it went wrong, as byte offsets into the expression.
    pub span: Span,
}

impl ParseError {
    pub(crate) fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            message: message.into(),
            span,
        }
    }

    /// Render the error with the source line and a caret marking the span,
    /// flecs-style:
    ///
    /// ```text
    /// error: unexpected ')'
    ///  | Position, Velocity)
    ///  |                   ^
    /// ```
    #[must_use]
    pub fn render(&self, src: &str) -> String {
        let mut start = self.span.start.min(src.len());
        // Spans are produced on char boundaries, but render takes arbitrary
        // (span, src) pairs; snap rather than let the slices below panic.
        while !src.is_char_boundary(start) {
            start -= 1;
        }
        let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
        let line_end = src[start..]
            .find('\n')
            .map_or(src.len(), |i| start + i);
        let line = &src[line_start..line_end];
        let column = src[line_start..start].chars().count();
        let caret = " ".repeat(column);
        format!(
            "error: {message}\n | {line}\n | {caret}^",
            message = self.message
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{message} (at byte {start})",
            message = self.message,
            start = self.span.start
        )
    }
}

impl std::error::Error for ParseError {}
