//! Tokenizer for the query DSL, mirroring flecs's `addons/parser/tokenizer.c`
//! as configured by the query parser: newlines are insignificant whitespace,
//! `//` and `/* */` comments are skipped, and identifiers merge `.` lookup
//! paths, keep `\.` escapes, and copy balanced `<...>` template arguments.

use crate::error::{ParseError, Span};

/// A lexed token kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// Identifier-like token: names, `$vars`, `#ids`, lookup paths.
    Ident,
    /// An integer literal (optionally negative).
    Number,
    /// A double-quoted string (value stored unescaped).
    Str,
    /// `*`
    Star,
    /// `,`
    Comma,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `!`
    Bang,
    /// `?`
    Question,
    /// `|`
    Pipe,
    /// `||`
    OrOr,
    /// `==`
    EqEq,
    /// `!=`
    Neq,
    /// `~=`
    Match,
    /// `@`
    At,
    /// End of input.
    Eof,
}

impl TokenKind {
    /// Human-readable description for error messages.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Ident => "identifier",
            Self::Number => "number",
            Self::Str => "string",
            Self::Star => "'*'",
            Self::Comma => "','",
            Self::LParen => "'('",
            Self::RParen => "')'",
            Self::LBracket => "'['",
            Self::RBracket => "']'",
            Self::LBrace => "'{'",
            Self::RBrace => "'}'",
            Self::Bang => "'!'",
            Self::Question => "'?'",
            Self::Pipe => "'|'",
            Self::OrOr => "'||'",
            Self::EqEq => "'=='",
            Self::Neq => "'!='",
            Self::Match => "'~='",
            Self::At => "'@'",
            Self::Eof => "end of expression",
        }
    }
}

/// A token with its unescaped text and source span.
#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub span: Span,
}

/// Lex the whole expression. Newlines and comments are skipped, so the
/// resulting stream contains only the tokens the term grammar consumes.
pub fn lex(src: &str) -> Result<Vec<Token>, ParseError> {
    let bytes = src.as_bytes();
    let mut tokens = Vec::new();
    let mut pos = 0;

    loop {
        pos = skip_trivia(bytes, pos)?;
        let Some(&c) = bytes.get(pos) else {
            tokens.push(Token {
                kind: TokenKind::Eof,
                text: String::new(),
                span: Span::at(pos),
            });
            return Ok(tokens);
        };

        let start = pos;
        let token = match c {
            b',' => punct(TokenKind::Comma, start, &mut pos),
            b'(' => punct(TokenKind::LParen, start, &mut pos),
            b')' => punct(TokenKind::RParen, start, &mut pos),
            b'[' => punct(TokenKind::LBracket, start, &mut pos),
            b']' => punct(TokenKind::RBracket, start, &mut pos),
            b'{' => punct(TokenKind::LBrace, start, &mut pos),
            b'}' => punct(TokenKind::RBrace, start, &mut pos),
            b'?' => punct(TokenKind::Question, start, &mut pos),
            b'@' => punct(TokenKind::At, start, &mut pos),
            b'*' => punct(TokenKind::Star, start, &mut pos),
            b'|' if bytes.get(pos + 1) == Some(&b'|') => {
                wide_punct(TokenKind::OrOr, start, &mut pos)
            }
            b'|' => punct(TokenKind::Pipe, start, &mut pos),
            b'=' if bytes.get(pos + 1) == Some(&b'=') => {
                wide_punct(TokenKind::EqEq, start, &mut pos)
            }
            b'!' if bytes.get(pos + 1) == Some(&b'=') => wide_punct(TokenKind::Neq, start, &mut pos),
            b'!' => punct(TokenKind::Bang, start, &mut pos),
            b'~' if bytes.get(pos + 1) == Some(&b'=') => {
                wide_punct(TokenKind::Match, start, &mut pos)
            }
            b'"' => lex_string(src, start, &mut pos)?,
            b'-' if bytes.get(pos + 1).is_some_and(u8::is_ascii_digit) => {
                lex_number(src, start, &mut pos)
            }
            _ if c.is_ascii_digit() => lex_number(src, start, &mut pos),
            _ if is_ident_start(c) => lex_ident(src, start, &mut pos)?,
            _ => {
                // Decode the full char so a multi-byte offender renders
                // intact in the message instead of as its lead byte.
                let offender = src[pos..].chars().next().expect("in-bounds char");
                return Err(ParseError::new(
                    format!("unknown token '{offender}'"),
                    Span::at(pos),
                ));
            }
        };
        tokens.push(token);
    }
}

/// Skip whitespace (including newlines) and `//` / `/* */` comments.
fn skip_trivia(bytes: &[u8], mut pos: usize) -> Result<usize, ParseError> {
    loop {
        while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        if bytes.get(pos) == Some(&b'/') && bytes.get(pos + 1) == Some(&b'/') {
            while bytes.get(pos).is_some_and(|&c| c != b'\n') {
                pos += 1;
            }
            continue;
        }
        if bytes.get(pos) == Some(&b'/') && bytes.get(pos + 1) == Some(&b'*') {
            let comment_start = pos;
            pos += 2;
            loop {
                if pos >= bytes.len() {
                    return Err(ParseError::new(
                        "missing */ for multiline comment",
                        Span::at(comment_start),
                    ));
                }
                if bytes[pos] == b'*' && bytes.get(pos + 1) == Some(&b'/') {
                    pos += 2;
                    break;
                }
                pos += 1;
            }
            continue;
        }
        return Ok(pos);
    }
}

const fn punct(kind: TokenKind, start: usize, pos: &mut usize) -> Token {
    *pos += 1;
    Token {
        kind,
        text: String::new(),
        span: Span {
            start,
            end: *pos,
        },
    }
}

const fn wide_punct(kind: TokenKind, start: usize, pos: &mut usize) -> Token {
    *pos += 2;
    Token {
        kind,
        text: String::new(),
        span: Span {
            start,
            end: *pos,
        },
    }
}

fn lex_number(src: &str, start: usize, pos: &mut usize) -> Token {
    let bytes = src.as_bytes();
    if bytes[*pos] == b'-' {
        *pos += 1;
    }
    while bytes.get(*pos).is_some_and(u8::is_ascii_digit) {
        *pos += 1;
    }
    Token {
        kind: TokenKind::Number,
        text: src[start..*pos].to_owned(),
        span: Span {
            start,
            end: *pos,
        },
    }
}

fn lex_string(src: &str, start: usize, pos: &mut usize) -> Result<Token, ParseError> {
    let bytes = src.as_bytes();
    *pos += 1;
    let mut text = String::new();
    loop {
        match bytes.get(*pos) {
            None => {
                return Err(ParseError::new("unterminated string", Span::at(start)));
            }
            Some(b'"') => {
                *pos += 1;
                return Ok(Token {
                    kind: TokenKind::Str,
                    text,
                    span: Span {
                        start,
                        end: *pos,
                    },
                });
            }
            Some(b'\\') => {
                // Decode the escaped character whole so a multi-byte char
                // is kept intact and `pos` never lands mid-codepoint.
                if let Some(escaped) = src[*pos + 1..].chars().next() {
                    text.push(escaped);
                    *pos += 1 + escaped.len_utf8();
                } else {
                    return Err(ParseError::new("unterminated string", Span::at(start)));
                }
            }
            Some(_) => {
                let ch = src[*pos..].chars().next().expect("in-bounds char");
                text.push(ch);
                *pos += ch.len_utf8();
            }
        }
    }
}

const fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || matches!(c, b'_' | b'$' | b'#')
}

const fn is_ident_continue(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// Lex an identifier following the upstream rules: `.` merges lookup paths
/// (`..` does not), `\.` is kept verbatim, any other `\x` escape resolves to
/// `x`, a `*` is kept only directly after a `.`, and `<...>` template
/// arguments are copied through balanced.
fn lex_ident(src: &str, start: usize, pos: &mut usize) -> Result<Token, ParseError> {
    let bytes = src.as_bytes();
    let mut text = String::new();

    while let Some(&c) = bytes.get(*pos) {
        if c == b'.' && bytes.get(*pos + 1) == Some(&b'.') {
            break;
        }

        if is_ident_continue(c) || c == b'.' {
            text.push(char::from(c));
            *pos += 1;
            continue;
        }

        match c {
            // Keep `\.` verbatim so lookup separators and literal dots in
            // names stay distinguishable; any other escape resolves.
            b'\\' if bytes.get(*pos + 1) == Some(&b'.') => {
                text.push_str("\\.");
                *pos += 2;
            }
            b'\\' => {
                // Same whole-char decoding as in strings: a multi-byte
                // escaped char must not split, or spans desynchronize.
                let Some(escaped) = src[*pos + 1..].chars().next() else {
                    // Upstream swallows a trailing backslash at end of input.
                    *pos += 1;
                    break;
                };
                text.push(escaped);
                *pos += 1 + escaped.len_utf8();
            }
            // Keep `.*` so wildcard lookup paths stay one token.
            b'*' if bytes[*pos - 1] == b'.' => {
                text.push('*');
                *pos += 1;
            }
            b'<' => {
                let mut depth = 0_usize;
                loop {
                    // Decode whole chars, as everywhere else: byte-wise
                    // copying turns multi-byte template arguments to mojibake.
                    let Some(t) = src[*pos..].chars().next() else {
                        return Err(ParseError::new(
                            "< without > in identifier",
                            Span::at(start),
                        ));
                    };
                    match t {
                        '<' => depth += 1,
                        '>' => depth -= 1,
                        _ => {}
                    }
                    text.push(t);
                    *pos += t.len_utf8();
                    if depth == 0 {
                        break;
                    }
                }
                break;
            }
            b'>' => {
                return Err(ParseError::new(
                    "> without < in identifier",
                    Span::at(*pos),
                ));
            }
            _ => break,
        }
    }

    Ok(Token {
        kind: TokenKind::Ident,
        text,
        span: Span {
            start,
            end: *pos,
        },
    })
}
