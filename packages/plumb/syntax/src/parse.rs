//! Hand-rolled recursive-descent parser. Character-level (no token stream):
//! quoting context decides what a byte means, so a separate lexer would need
//! the same state anyway.

use snafu::Snafu;

use crate::ast::{
    AndOr, AndOrTail, Assign, Command, Connector, Item, Part, Pipeline, Program, RedirOp,
    Redirect, Span, Word,
};

/// Parse failure: an unsupported construct or malformed input, with the byte
/// span of the offending text.
#[derive(Debug, Snafu)]
#[snafu(display("parse error at byte {}: {message}", span.start))]
pub struct ParseError {
    /// What went wrong, naming the unsupported construct where relevant.
    pub message: String,
    /// Byte range of the offending source text.
    pub span: Span,
}

/// Bash keywords and compound-command openers the subset refuses at command
/// position; running them as argv[0] would silently mean something else than
/// it does in bash.
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "select", "coproc", "time", "in", "!", "[[", "]]",
];

/// Parse a source string into a [`Program`].
///
/// # Errors
///
/// Returns [`ParseError`] on malformed input or on any bash construct outside
/// the plumb subset (keywords, subshells, backticks, here-docs, brace or
/// fancy parameter expansion, positional parameters).
pub fn parse(src: &str) -> Result<Program, ParseError> {
    let mut parser = Parser::new(src);
    let program = parser.program(None)?;
    if let Some(unexpected) = parser.peek() {
        return parser.fail_at(parser.byte_pos(), format!("unexpected `{unexpected}`"));
    }
    Ok(program)
}

struct Parser<'src> {
    src: &'src str,
    chars: Vec<CharAt>,
    pos: usize,
}

#[derive(Clone, Copy)]
struct CharAt {
    byte: usize,
    ch: char,
}

/// Word-part accumulator: merges runs of literal text that share a quoting
/// context and remembers whether any quotes appeared (so `''` still yields an
/// empty argument).
#[derive(Default)]
struct WordAcc {
    parts: Vec<Part>,
    pending: String,
    pending_quoted: bool,
    saw_quotes: bool,
}

impl WordAcc {
    fn push(&mut self, c: char, quoted: bool) {
        if self.pending_quoted != quoted && !self.pending.is_empty() {
            self.flush();
        }
        self.pending_quoted = quoted;
        self.pending.push(c);
    }

    fn push_part(&mut self, part: Part) {
        self.flush();
        self.parts.push(part);
    }

    fn flush(&mut self) {
        if !self.pending.is_empty() {
            self.parts.push(Part::Text {
                text: std::mem::take(&mut self.pending),
                quoted: self.pending_quoted,
            });
        }
    }

    const fn is_empty(&self) -> bool {
        self.parts.is_empty() && self.pending.is_empty()
    }

    fn finish(mut self) -> Vec<Part> {
        self.flush();
        if self.parts.is_empty() && self.saw_quotes {
            self.parts.push(Part::Text {
                text: String::new(),
                quoted: true,
            });
        }
        self.parts
    }
}

impl<'src> Parser<'src> {
    fn new(src: &'src str) -> Self {
        Self {
            src,
            chars: src
                .char_indices()
                .map(|(byte, ch)| CharAt { byte, ch })
                .collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).map(|c| c.ch)
    }

    fn peek_at(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).map(|c| c.ch)
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn byte_pos(&self) -> usize {
        self.chars.get(self.pos).map_or(self.src.len(), |c| c.byte)
    }

    fn span_from(&self, start: usize) -> Span {
        Span {
            start,
            end: self.byte_pos(),
        }
    }

    fn fail_at<T>(&self, start: usize, message: String) -> Result<T, ParseError> {
        ParseSnafu {
            message,
            span: Span {
                start,
                end: self.byte_pos().max(start + 1),
            },
        }
        .fail()
    }

    /// Skip spaces, tabs, backslash-newline continuations, and comments.
    /// Comments only start at token boundaries, which is exactly where this
    /// is called, matching bash.
    fn skip_blank(&mut self) {
        loop {
            match self.peek() {
                Some(' ' | '\t') => {
                    self.pos += 1;
                }
                Some('\\') if self.peek_at(1) == Some('\n') => {
                    self.pos += 2;
                }
                Some('#') => {
                    while !matches!(self.peek(), None | Some('\n')) {
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    fn skip_blank_and_newlines(&mut self) {
        loop {
            self.skip_blank();
            if self.peek() == Some('\n') {
                self.pos += 1;
            } else {
                return;
            }
        }
    }

    /// `program := item*` (until EOF or `stop`)
    fn program(&mut self, stop: Option<char>) -> Result<Program, ParseError> {
        let mut items = Vec::new();
        loop {
            self.skip_blank_and_newlines();
            match self.peek() {
                None => return Ok(Program { items }),
                Some(c) if Some(c) == stop => return Ok(Program { items }),
                _ => {}
            }
            items.push(self.item(stop)?);
        }
    }

    /// `item := and_or ('&' | ';' | newline)?`
    fn item(&mut self, stop: Option<char>) -> Result<Item, ParseError> {
        let and_or = self.and_or(stop)?;
        self.skip_blank();
        if self.peek() == Some('&') && self.peek_at(1) != Some('&') {
            // `&` is itself the item separator: `a & b` runs `a` in the
            // background and continues with `b` immediately.
            self.pos += 1;
            return Ok(Item {
                and_or,
                background: true,
            });
        }
        match self.peek() {
            Some(';') => {
                if self.peek_at(1) == Some(';') {
                    return self.fail_at(self.byte_pos(), "unsupported: `;;`".to_owned());
                }
                self.pos += 1;
            }
            Some('\n') => {
                self.pos += 1;
            }
            None => {}
            Some(c) if Some(c) == stop => {}
            Some(c) => {
                return self.fail_at(self.byte_pos(), format!("unexpected `{c}`"));
            }
        }
        Ok(Item {
            and_or,
            background: false,
        })
    }

    /// `and_or := pipeline (('&&' | '||') pipeline)*`
    fn and_or(&mut self, stop: Option<char>) -> Result<AndOr, ParseError> {
        self.skip_blank();
        let start = self.byte_pos();
        let first = self.pipeline(stop)?;
        let mut rest = Vec::new();
        loop {
            self.skip_blank();
            let connector = match (self.peek(), self.peek_at(1)) {
                (Some('&'), Some('&')) => Connector::And,
                (Some('|'), Some('|')) => Connector::Or,
                _ => break,
            };
            self.pos += 2;
            self.skip_blank_and_newlines();
            let pipeline = self.pipeline(stop)?;
            rest.push(AndOrTail {
                connector,
                pipeline,
            });
        }
        Ok(AndOr {
            first,
            rest,
            span: self.span_from(start),
        })
    }

    /// `pipeline := command ('|' command)*`
    fn pipeline(&mut self, stop: Option<char>) -> Result<Pipeline, ParseError> {
        self.skip_blank();
        let start = self.byte_pos();
        let mut commands = vec![self.command(stop)?];
        loop {
            self.skip_blank();
            match (self.peek(), self.peek_at(1)) {
                (Some('|'), Some('|')) => break,
                (Some('|'), Some('&')) => {
                    return self
                        .fail_at(self.byte_pos(), "unsupported: `|&`; use `2>&1 |`".to_owned());
                }
                (Some('|'), _) => {
                    self.pos += 1;
                    self.skip_blank_and_newlines();
                    commands.push(self.command(stop)?);
                }
                _ => break,
            }
        }
        Ok(Pipeline {
            commands,
            span: self.span_from(start),
        })
    }

    /// `command := assign* (word | redirect)+`
    fn command(&mut self, stop: Option<char>) -> Result<Command, ParseError> {
        self.skip_blank();
        let start = self.byte_pos();
        let mut assigns = Vec::new();
        let mut words: Vec<Word> = Vec::new();
        let mut redirects = Vec::new();
        loop {
            self.skip_blank();
            let element_start = self.byte_pos();
            match self.peek() {
                None | Some('\n' | ';' | '|') => break,
                Some(c) if Some(c) == stop => break,
                Some('&') if !matches!(self.peek_at(1), Some('>')) => break,
                Some('(') => {
                    return self.fail_at(
                        element_start,
                        "unsupported: subshell / function definition".to_owned(),
                    );
                }
                _ => {}
            }
            if let Some(redirect) = self.try_redirect()? {
                redirects.push(redirect);
                continue;
            }
            if words.is_empty()
                && let Some(assign) = self.try_assign()?
            {
                assigns.push(assign);
                continue;
            }
            let word = self.word()?;
            if words.is_empty()
                && let Some(keyword) = unquoted_literal(&word)
                && KEYWORDS.contains(&keyword.as_str())
            {
                return self.fail_at(
                    element_start,
                    format!("unsupported: bash keyword `{keyword}`"),
                );
            }
            words.push(word);
        }
        if assigns.is_empty() && words.is_empty() && redirects.is_empty() {
            return self.fail_at(start, "expected a command".to_owned());
        }
        Ok(Command {
            assigns,
            words,
            redirects,
            span: self.span_from(start),
        })
    }
}

impl Parser<'_> {
    /// Recognize a redirection operator at the current position, if any.
    /// Must be called at a token boundary: a digit only introduces a
    /// redirect when `>` follows immediately, so `2 > f` is the word `2`
    /// plus a stdout redirect, same as bash.
    fn try_redirect(&mut self) -> Result<Option<Redirect>, ParseError> {
        let start = self.byte_pos();
        let (op, len) = match (self.peek(), self.peek_at(1), self.peek_at(2), self.peek_at(3)) {
            (Some('2'), Some('>'), Some('&'), Some('1')) => (RedirOp::ErrToOut, 4),
            (Some('1'), Some('>'), Some('&'), Some('2')) => (RedirOp::OutToErr, 4),
            (Some('>'), Some('&'), Some('2'), _) => (RedirOp::OutToErr, 3),
            (Some('>'), Some('&'), next, _) => {
                let shown = next.map_or(String::new(), |c| c.to_string());
                return self.fail_at(start, format!("unsupported redirection `>&{shown}`"));
            }
            (Some('2'), Some('>'), Some('>'), _) => (RedirOp::ErrAppend, 3),
            (Some('2'), Some('>'), _, _) => (RedirOp::ErrTrunc, 2),
            (Some('1'), Some('>'), Some('>'), _) => (RedirOp::OutAppend, 3),
            (Some('1'), Some('>'), _, _) => (RedirOp::OutTrunc, 2),
            (Some('&'), Some('>'), Some('>'), _) => (RedirOp::BothAppend, 3),
            (Some('&'), Some('>'), _, _) => (RedirOp::BothTrunc, 2),
            (Some('>'), Some('>'), _, _) => (RedirOp::OutAppend, 2),
            (Some('>'), _, _, _) => (RedirOp::OutTrunc, 1),
            (Some('<'), Some('<'), _, _) => {
                return self.fail_at(start, "unsupported: here-doc / here-string".to_owned());
            }
            (Some('<'), Some('('), _, _) => {
                return self.fail_at(start, "unsupported: process substitution".to_owned());
            }
            (Some('<'), _, _, _) => (RedirOp::In, 1),
            _ => return Ok(None),
        };
        self.pos += len;
        let target = if matches!(op, RedirOp::ErrToOut | RedirOp::OutToErr) {
            None
        } else {
            self.skip_blank();
            match self.peek() {
                None | Some('\n' | ';' | '|' | '&' | '<' | '>') => {
                    return self
                        .fail_at(start, "redirection is missing its file target".to_owned());
                }
                _ => Some(self.word()?),
            }
        };
        Ok(Some(Redirect {
            op,
            target,
            span: self.span_from(start),
        }))
    }

    /// Recognize `NAME=` at the current position and parse the value word.
    fn try_assign(&mut self) -> Result<Option<Assign>, ParseError> {
        let start_pos = self.pos;
        let start = self.byte_pos();
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c == '_' || c.is_ascii_alphabetic() || (!name.is_empty() && c.is_ascii_digit()) {
                name.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        if name.is_empty() || self.peek() != Some('=') {
            self.pos = start_pos;
            return Ok(None);
        }
        self.pos += 1;
        let value = if matches!(self.peek(), None | Some(' ' | '\t' | '\n' | ';' | '|' | '&')) {
            Word {
                parts: Vec::new(),
                span: self.span_from(self.byte_pos()),
            }
        } else {
            self.word()?
        };
        Ok(Some(Assign {
            name,
            value,
            span: self.span_from(start),
        }))
    }

    /// `word := (run | quotes | escape | dollar-form)+`
    fn word(&mut self) -> Result<Word, ParseError> {
        let start = self.byte_pos();
        let mut acc = WordAcc::default();
        while let Some(c) = self.peek() {
            match c {
                ' ' | '\t' | '\n' | ';' | '|' | '&' | '<' | '>' | ')' => break,
                '(' => {
                    return self.fail_at(self.byte_pos(), "unsupported: `(` in a word".to_owned());
                }
                '`' => {
                    return self.fail_at(
                        self.byte_pos(),
                        "unsupported: backtick command substitution; use $(...)".to_owned(),
                    );
                }
                '{' | '}' => {
                    return self.fail_at(
                        self.byte_pos(),
                        format!("unsupported: brace expansion; quote the `{c}`"),
                    );
                }
                '\\' => {
                    self.pos += 1;
                    match self.bump() {
                        Some('\n') => {}
                        Some(escaped) => acc.push(escaped, true),
                        None => {
                            return self
                                .fail_at(self.byte_pos(), "trailing backslash".to_owned());
                        }
                    }
                }
                '\'' => self.single_quoted(&mut acc)?,
                '"' => self.double_quoted(&mut acc)?,
                '$' => {
                    if !self.dollar(&mut acc)? {
                        acc.push('$', false);
                    }
                }
                '~' if acc.is_empty() => {
                    // Bare `~` or `~/...` at word start is $HOME, same as
                    // bash; `~user` is out of the subset.
                    self.pos += 1;
                    match self.peek() {
                        None | Some(' ' | '\t' | '\n' | ';' | '|' | '&' | '<' | '>' | '/') => {
                            acc.push_part(Part::Var {
                                name: "HOME".to_owned(),
                                indices: Vec::new(),
                                span: Span {
                                    start,
                                    end: self.byte_pos(),
                                },
                            });
                        }
                        Some(_) => {
                            return self
                                .fail_at(start, "unsupported: `~user` expansion".to_owned());
                        }
                    }
                }
                _ => {
                    acc.push(c, false);
                    self.pos += 1;
                }
            }
        }
        let parts = acc.finish();
        if parts.is_empty() {
            return self.fail_at(start, "expected a word".to_owned());
        }
        Ok(Word {
            parts,
            span: self.span_from(start),
        })
    }

    fn single_quoted(&mut self, acc: &mut WordAcc) -> Result<(), ParseError> {
        let quote_start = self.byte_pos();
        self.pos += 1;
        acc.saw_quotes = true;
        loop {
            match self.bump() {
                Some('\'') => return Ok(()),
                Some(inner) => acc.push(inner, true),
                None => {
                    return self.fail_at(quote_start, "unterminated single quote".to_owned());
                }
            }
        }
    }

    fn double_quoted(&mut self, acc: &mut WordAcc) -> Result<(), ParseError> {
        let quote_start = self.byte_pos();
        self.pos += 1;
        acc.saw_quotes = true;
        loop {
            match self.peek() {
                None => {
                    return self.fail_at(quote_start, "unterminated double quote".to_owned());
                }
                Some('"') => {
                    self.pos += 1;
                    return Ok(());
                }
                Some('\\') => {
                    self.pos += 1;
                    match self.bump() {
                        Some(escaped @ ('$' | '"' | '\\' | '`')) => acc.push(escaped, true),
                        Some('\n') => {}
                        Some(other) => {
                            acc.push('\\', true);
                            acc.push(other, true);
                        }
                        None => {
                            return self
                                .fail_at(quote_start, "unterminated double quote".to_owned());
                        }
                    }
                }
                Some('`') => {
                    return self.fail_at(
                        self.byte_pos(),
                        "unsupported: backtick command substitution; use $(...)".to_owned(),
                    );
                }
                Some('$') => {
                    if !self.dollar(acc)? {
                        acc.push('$', true);
                    }
                }
                Some(inner) => {
                    acc.push(inner, true);
                    self.pos += 1;
                }
            }
        }
    }

    /// The braced form after `${`: a plain name, optionally indexed as a
    /// run reference (`${o[7]}`, `${o[7][0]}`, negative from the latest).
    fn braced(&mut self, acc: &mut WordAcc, start: usize) -> Result<bool, ParseError> {
        let name = self.ident_run();
        if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
            return self.fail_at(
                start,
                "unsupported parameter expansion: only a plain variable name in braces"
                    .to_owned(),
            );
        }
        let mut indices = Vec::new();
        while self.peek() == Some('[') {
            if indices.len() == 2 {
                return self.fail_at(
                    start,
                    "run references take at most two indexes: ${o[run][stage]}".to_owned(),
                );
            }
            self.pos += 1;
            let negative = self.peek() == Some('-');
            if negative {
                self.pos += 1;
            }
            let mut digits = String::new();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    digits.push(c);
                    self.pos += 1;
                } else {
                    break;
                }
            }
            let (Ok(magnitude), Some(']')) = (digits.parse::<i64>(), self.peek()) else {
                return self.fail_at(
                    start,
                    "run reference indexes are integers: ${o[7]} or ${o[-1]}".to_owned(),
                );
            };
            self.pos += 1;
            indices.push(if negative { -magnitude } else { magnitude });
        }
        if self.peek() != Some('}') {
            return self.fail_at(
                start,
                "unsupported parameter expansion: only a plain variable name in braces"
                    .to_owned(),
            );
        }
        self.pos += 1;
        acc.push_part(Part::Var {
            name,
            indices,
            span: Span {
                start,
                end: self.byte_pos(),
            },
        });
        Ok(true)
    }

    /// Consume a run of identifier characters (`[A-Za-z0-9_]*`).
    fn ident_run(&mut self) -> String {
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c == '_' || c.is_ascii_alphanumeric() {
                name.push(c);
                self.pos += 1;
            } else {
                break;
            }
        }
        name
    }

    /// Parse a `$`-introduced form; the `$` has not been consumed yet.
    /// Returns false when the `$` turns out to be literal (end of word or
    /// before plain punctuation), leaving the caller to emit it.
    fn dollar(&mut self, acc: &mut WordAcc) -> Result<bool, ParseError> {
        let start = self.byte_pos();
        match self.peek_at(1) {
            Some('(') if self.peek_at(2) == Some('(') => self.fail_at(
                start,
                "unsupported: arithmetic expansion $((...))".to_owned(),
            ),
            Some('(') => {
                self.pos += 2;
                let program = self.program(Some(')'))?;
                if self.bump() != Some(')') {
                    return self
                        .fail_at(start, "unterminated $( command substitution".to_owned());
                }
                acc.push_part(Part::CommandSub {
                    program,
                    span: Span {
                        start,
                        end: self.byte_pos(),
                    },
                });
                Ok(true)
            }
            Some('{') => {
                self.pos += 2;
                self.braced(acc, start)
            }
            Some('?') => {
                self.pos += 2;
                acc.push_part(Part::Var {
                    name: "?".to_owned(),
                    indices: Vec::new(),
                    span: Span {
                        start,
                        end: self.byte_pos(),
                    },
                });
                Ok(true)
            }
            Some(first) if first == '_' || first.is_ascii_alphabetic() => {
                self.pos += 1;
                let name = self.ident_run();
                acc.push_part(Part::Var {
                    name,
                    indices: Vec::new(),
                    span: Span {
                        start,
                        end: self.byte_pos(),
                    },
                });
                Ok(true)
            }
            Some(c) if c.is_ascii_digit() || "@*#!-$".contains(c) => {
                self.fail_at(start, format!("unsupported: special parameter `${c}`"))
            }
            _ => {
                self.pos += 1;
                Ok(false)
            }
        }
    }
}

/// The word's text when it is a single unquoted literal (keyword detection).
fn unquoted_literal(word: &Word) -> Option<String> {
    match word.parts.as_slice() {
        [Part::Text { text, quoted: false }] => Some(text.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, Command, Connector, Part, RedirOp};

    fn one_command(src: &str) -> Command {
        let program = parse(src).expect("should parse");
        assert_eq!(program.items.len(), 1, "one item in {src:?}");
        let item = program.items.into_iter().next().expect("item");
        assert!(item.and_or.rest.is_empty());
        item.and_or
            .first
            .commands
            .into_iter()
            .next()
            .expect("command")
    }

    fn literal_argv(src: &str) -> Vec<String> {
        one_command(src)
            .words
            .iter()
            .map(|w| {
                w.parts
                    .iter()
                    .map(|p| match p {
                        Part::Text { text, .. } => text.clone(),
                        other => panic!("expected literal, got {other:?}"),
                    })
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn simple_command() {
        assert_eq!(literal_argv("echo hello world"), ["echo", "hello", "world"]);
    }

    #[test]
    fn quoting_resolves() {
        assert_eq!(
            literal_argv(r#"printf "a b" 'c  d' e\ f"#),
            ["printf", "a b", "c  d", "e f"]
        );
    }

    #[test]
    fn empty_quotes_are_an_empty_argument() {
        assert_eq!(literal_argv("printf ''"), ["printf", ""]);
    }

    #[test]
    fn pipeline_and_connectors() {
        let program = parse("a | b | c && d || e; f\ng &").expect("should parse");
        assert_eq!(program.items.len(), 3);
        assert_eq!(program.items[0].and_or.first.commands.len(), 3);
        assert_eq!(program.items[0].and_or.rest.len(), 2);
        assert_eq!(program.items[0].and_or.rest[0].connector, Connector::And);
        assert_eq!(program.items[0].and_or.rest[1].connector, Connector::Or);
        assert!(!program.items[0].background);
        assert!(program.items[2].background);
    }

    #[test]
    fn background_then_next_item() {
        let program = parse("sleep 5 & echo hi").expect("should parse");
        assert_eq!(program.items.len(), 2);
        assert!(program.items[0].background);
        assert!(!program.items[1].background);
    }

    #[test]
    fn variables() {
        let command = one_command("echo $FOO ${BAR} $?");
        let names: Vec<&str> = command.words[1..]
            .iter()
            .map(|w| match &w.parts[0] {
                Part::Var { name, .. } => name.as_str(),
                other => panic!("expected var, got {other:?}"),
            })
            .collect();
        assert_eq!(names, ["FOO", "BAR", "?"]);
    }

    /// Each source must fail to parse with a message naming the construct.
    fn assert_parse_errors(cases: &[(&str, &str)]) {
        for (src, needle) in cases {
            let error = parse(src).expect_err(src);
            assert!(
                error.message.contains(needle),
                "{src:?}: {} should mention {needle:?}",
                error.message
            );
        }
    }

    #[test]
    fn run_references() {
        let command = one_command("echo ${o[7]} ${e[7][0]} ${s[-1]}");
        let refs: Vec<(&str, &[i64])> = command.words[1..]
            .iter()
            .map(|w| match &w.parts[0] {
                Part::Var { name, indices, .. } => (name.as_str(), indices.as_slice()),
                other => panic!("expected var, got {other:?}"),
            })
            .collect();
        assert_eq!(
            refs,
            [
                ("o", [7_i64].as_slice()),
                ("e", [7_i64, 0].as_slice()),
                ("s", [-1_i64].as_slice())
            ]
        );
    }

    #[test]
    fn run_reference_errors() {
        assert_parse_errors(&[
            ("echo ${o[1][2][3]}", "at most two"),
            ("echo ${o[x]}", "integers"),
            ("echo ${o[1}", "integers"),
            ("echo ${o[]}", "integers"),
        ]);
    }

    #[test]
    fn tilde_is_home() {
        let command = one_command("ls ~/src");
        match &command.words[1].parts[0] {
            Part::Var { name, .. } => assert_eq!(name, "HOME"),
            other => panic!("expected var, got {other:?}"),
        }
    }

    #[test]
    fn command_substitution_nests() {
        let command = one_command("echo $(basename $(pwd))");
        match &command.words[1].parts[0] {
            Part::CommandSub { program, .. } => {
                let inner = &program.items[0].and_or.first.commands[0];
                match &inner.words[1].parts[0] {
                    Part::CommandSub { .. } => {}
                    other => panic!("expected nested substitution, got {other:?}"),
                }
            }
            other => panic!("expected substitution, got {other:?}"),
        }
    }

    #[test]
    fn command_substitution_in_double_quotes() {
        let command = one_command(r#"echo "now: $(date)""#);
        assert!(
            command.words[1]
                .parts
                .iter()
                .any(|p| matches!(p, Part::CommandSub { .. }))
        );
    }

    #[test]
    fn redirections() {
        let command = one_command("cmd > out.txt 2>> err.txt < in.txt 2>&1");
        let ops: Vec<RedirOp> = command.redirects.iter().map(|r| r.op).collect();
        assert_eq!(
            ops,
            [
                RedirOp::OutTrunc,
                RedirOp::ErrAppend,
                RedirOp::In,
                RedirOp::ErrToOut
            ]
        );
        assert!(command.redirects[3].target.is_none());
    }

    #[test]
    fn both_redirect_and_stderr_shorthand() {
        assert_eq!(
            one_command("cmd &> all.log").redirects[0].op,
            RedirOp::BothTrunc
        );
        assert_eq!(one_command("echo oops >&2").redirects[0].op, RedirOp::OutToErr);
    }

    #[test]
    fn assignments() {
        let command = one_command("FOO=bar BAZ= cmd arg");
        assert_eq!(command.assigns.len(), 2);
        assert_eq!(command.assigns[0].name, "FOO");
        assert!(command.assigns[1].value.parts.is_empty());
        assert_eq!(command.words.len(), 2);

        let bare = one_command("X=1");
        assert_eq!(bare.assigns.len(), 1);
        assert!(bare.words.is_empty());
    }

    #[test]
    fn comments_ignored() {
        let program = parse("echo hi # trailing\n# whole line\necho bye").expect("should parse");
        assert_eq!(program.items.len(), 2);
    }

    #[test]
    fn digit_word_is_not_a_redirect() {
        assert_eq!(literal_argv("echo 2"), ["echo", "2"]);
        let redirected = one_command("echo 2> f");
        assert_eq!(redirected.words.len(), 1);
        assert_eq!(redirected.redirects[0].op, RedirOp::ErrTrunc);
    }

    #[test]
    fn unsupported_constructs_error_loudly() {
        let fancy_expansion = concat!("echo ${", "X:-default}");
        assert_parse_errors(&[
            ("if true; then echo hi; fi", "keyword `if`"),
            ("for x in a b; do echo $x; done", "keyword `for`"),
            ("echo `date`", "backtick"),
            ("cat << EOF", "here-doc"),
            ("echo $((1+2))", "arithmetic"),
            (fancy_expansion, "parameter expansion"),
            ("echo $1", "special parameter"),
            ("echo $@", "special parameter"),
            ("echo {a,b}", "brace expansion"),
            ("(cd /tmp && ls)", "subshell"),
            ("ls ~user", "~user"),
            ("a |& b", "|&"),
            ("case x in esac", "keyword `case`"),
            ("diff <(sort a) <(sort b)", "process substitution"),
        ]);
    }

    #[test]
    fn spans_point_into_source() {
        let src = "echo `date`";
        let error = parse(src).expect_err("backtick");
        assert_eq!(&src[error.span.start..=error.span.start], "`");
    }

    #[test]
    fn empty_input_is_empty_program() {
        assert!(parse("").expect("empty ok").items.is_empty());
        assert!(parse("  \n # comment\n").expect("blank ok").items.is_empty());
    }
}
