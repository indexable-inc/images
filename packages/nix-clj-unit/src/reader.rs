//! A focused reader for the slice of Clojure syntax an `ns` form can contain.
//!
//! This is deliberately not an EDN library. `ns` forms lean on reader macros
//! EDN does not define -- `#?` reader conditionals, `#_` discard, `^meta`,
//! character and regex literals -- and a parser that drops any one of them
//! turns a legal source file into a bogus "namespace with no edges". What is
//! here reads far enough to find the `ns` form and understand its `:require`
//! clauses; everything whose *value* the ns reader never inspects (numbers,
//! characters, regexes) collapses to [`Kind::Atom`], which keeps the reader
//! small without making it guess.
//!
//! Every form carries the byte offset it started at, so a malformed file is
//! reported at a position rather than as a shrug.

use std::collections::VecDeque;
use std::fmt;

/// The Clojure feature this reader resolves `#?` conditionals against. Units
/// are AOT-compiled by the JVM `clojure.main`, so `:cljs` branches are dead
/// code here and are read-but-discarded.
const FEATURE: &str = "clj";

/// `:default` matches whatever the active feature is, and always last.
const FEATURE_DEFAULT: &str = "default";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Symbol(String),
    /// Keyword text with the leading `:` removed, so `:require` is `"require"`
    /// and the auto-resolved `::require` is `":require"` -- distinct on
    /// purpose, because they mean different things.
    Keyword(String),
    Str(String),
    List(Vec<Form>),
    Vector(Vec<Form>),
    Map(Vec<Form>),
    Set(Vec<Form>),
    /// Syntax the `ns` reader never looks inside: numbers, characters, regexes,
    /// `#=` eval forms.
    Atom,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub offset: usize,
    pub kind: Kind,
}

impl Form {
    pub fn symbol(&self) -> Option<&str> {
        match &self.kind {
            Kind::Symbol(name) => Some(name),
            _ => None,
        }
    }

    pub fn keyword(&self) -> Option<&str> {
        match &self.kind {
            Kind::Keyword(name) => Some(name),
            _ => None,
        }
    }

    /// A one-word name for this form's shape, for error messages.
    pub const fn describe(&self) -> &'static str {
        match self.kind {
            Kind::Symbol(_) => "a symbol",
            Kind::Keyword(_) => "a keyword",
            Kind::Str(_) => "a string",
            Kind::List(_) => "a list",
            Kind::Vector(_) => "a vector",
            Kind::Map(_) => "a map",
            Kind::Set(_) => "a set",
            Kind::Atom => "a literal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadError {
    pub offset: usize,
    pub message: String,
}

impl ReadError {
    pub fn new(offset: usize, message: impl Into<String>) -> Self {
        Self {
            offset,
            message: message.into(),
        }
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "byte offset {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ReadError {}

/// What one read step produced. A step is not always one form: `#_` discards,
/// and `#?@` splices several forms into the enclosing collection.
enum Item {
    One(Form),
    Splice(Vec<Form>),
    Nothing,
}

pub struct Reader<'a> {
    src: &'a str,
    pos: usize,
    /// Forms a top-level `#?@` spliced out, waiting to be handed back one at a
    /// time by [`Reader::next_form`].
    pending: VecDeque<Form>,
}

impl<'a> Reader<'a> {
    pub const fn new(src: &'a str) -> Self {
        Self {
            src,
            pos: 0,
            pending: VecDeque::new(),
        }
    }

    /// The next top-level form, or `None` at end of input.
    pub fn next_form(&mut self) -> Result<Option<Form>, ReadError> {
        loop {
            if let Some(form) = self.pending.pop_front() {
                return Ok(Some(form));
            }
            match self.read_item()? {
                None => return Ok(None),
                Some(Item::One(form)) => return Ok(Some(form)),
                Some(Item::Splice(forms)) => self.pending.extend(forms),
                Some(Item::Nothing) => {}
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn peek_second(&self) -> Option<char> {
        let mut chars = self.src[self.pos..].chars();
        chars.next()?;
        chars.next()
    }

    fn bump(&mut self) -> Option<char> {
        let next = self.peek()?;
        self.pos += next.len_utf8();
        Some(next)
    }

    fn skip_line(&mut self) {
        while let Some(next) = self.bump() {
            if next == '\n' {
                break;
            }
        }
    }

    /// Whitespace, commas (whitespace in Clojure) and `;` line comments,
    /// including `;;` inside an `ns` form.
    fn skip_ignorable(&mut self) {
        while let Some(next) = self.peek() {
            if next.is_whitespace() || next == ',' {
                self.bump();
            } else if next == ';' {
                self.skip_line();
            } else {
                break;
            }
        }
    }

    /// Read exactly one form, refusing the two step outcomes that cannot stand
    /// in for a form (a `#_` discard, a `#?@` splice).
    fn require_form(&mut self, at: usize, what: &str) -> Result<Form, ReadError> {
        loop {
            match self.read_item()? {
                None => return Err(ReadError::new(at, format!("end of input while reading {what}"))),
                Some(Item::One(form)) => return Ok(form),
                Some(Item::Nothing) => {}
                Some(Item::Splice(_)) => {
                    return Err(ReadError::new(
                        at,
                        format!("a splicing reader conditional cannot supply {what}"),
                    ));
                }
            }
        }
    }

    fn read_item(&mut self) -> Result<Option<Item>, ReadError> {
        self.skip_ignorable();
        let start = self.pos;
        let Some(next) = self.peek() else {
            return Ok(None);
        };

        let form_kind = match next {
            '(' => {
                self.bump();
                Kind::List(self.read_collection(start, ')')?)
            }
            '[' => {
                self.bump();
                Kind::Vector(self.read_collection(start, ']')?)
            }
            '{' => {
                self.bump();
                Kind::Map(self.read_collection(start, '}')?)
            }
            ')' | ']' | '}' => {
                return Err(ReadError::new(start, format!("unbalanced `{next}`")));
            }
            '"' => Kind::Str(self.read_string()?),
            '\\' => {
                self.read_character()?;
                Kind::Atom
            }
            ':' => Kind::Keyword(self.read_keyword()),
            // Quoting and deref wrap a form without changing which namespaces it
            // names, so the wrapper is dropped and the inner form returned.
            '\'' | '`' | '@' => {
                self.bump();
                return Ok(Some(Item::One(self.require_form(start, "a quoted form")?)));
            }
            '~' => {
                self.bump();
                if self.peek() == Some('@') {
                    self.bump();
                }
                return Ok(Some(Item::One(self.require_form(start, "an unquoted form")?)));
            }
            '^' => {
                self.bump();
                self.require_form(start, "a metadata form")?;
                return Ok(Some(Item::One(
                    self.require_form(start, "the form a metadata marker applies to")?,
                )));
            }
            '#' => return self.read_dispatch(start).map(Some),
            _ => self.read_token_form(start)?,
        };

        Ok(Some(Item::One(Form {
            offset: start,
            kind: form_kind,
        })))
    }

    fn read_collection(&mut self, open_at: usize, close: char) -> Result<Vec<Form>, ReadError> {
        let mut items = Vec::new();
        loop {
            self.skip_ignorable();
            match self.peek() {
                None => {
                    return Err(ReadError::new(
                        open_at,
                        format!("unclosed form, expected a `{close}`"),
                    ));
                }
                Some(next) if next == close => {
                    self.bump();
                    return Ok(items);
                }
                Some(next @ (')' | ']' | '}')) => {
                    return Err(ReadError::new(
                        self.pos,
                        format!(
                            "found `{next}` where `{close}` was expected, closing the form \
                             opened at byte offset {open_at}"
                        ),
                    ));
                }
                Some(_) => match self.read_item()? {
                    Some(Item::One(form)) => items.push(form),
                    Some(Item::Splice(forms)) => items.extend(forms),
                    Some(Item::Nothing) | None => {}
                },
            }
        }
    }

    fn read_dispatch(&mut self, start: usize) -> Result<Item, ReadError> {
        self.bump();
        let Some(next) = self.peek() else {
            return Err(ReadError::new(start, "end of input after `#`"));
        };

        let form_kind = match next {
            '{' => {
                self.bump();
                Kind::Set(self.read_collection(start, '}')?)
            }
            // `#(...)`, an anonymous function; its body is an ordinary list.
            '(' => {
                self.bump();
                Kind::List(self.read_collection(start, ')')?)
            }
            '_' => {
                self.bump();
                self.require_form(start, "the form `#_` discards")?;
                return Ok(Item::Nothing);
            }
            '?' => return self.read_conditional(start),
            // `#'var-quote` and `#=(eval)` both wrap one form.
            '\'' => {
                self.bump();
                return Ok(Item::One(self.require_form(start, "a var-quoted form")?));
            }
            '=' => {
                self.bump();
                self.require_form(start, "an eval form")?;
                Kind::Atom
            }
            '"' => {
                self.read_regex()?;
                Kind::Atom
            }
            // `#!` is a shebang line, legal at the top of a script.
            '!' => {
                self.skip_line();
                return Ok(Item::Nothing);
            }
            // `#:prefix{...}` / `#::{...}`, a namespaced map literal.
            ':' => {
                self.read_keyword();
                return Ok(Item::One(
                    self.require_form(start, "the map of a namespaced map literal")?,
                ));
            }
            // `#inst "..."`, `#uuid "..."`, or any user tag: read the tag, then
            // hand back the tagged form itself.
            _ if is_token_char(next) => {
                self.read_token();
                return Ok(Item::One(
                    self.require_form(start, "the value of a tagged literal")?,
                ));
            }
            _ => {
                return Err(ReadError::new(
                    start,
                    format!("unsupported dispatch macro `#{next}`"),
                ));
            }
        };

        Ok(Item::One(Form {
            offset: start,
            kind: form_kind,
        }))
    }

    /// `#?(:clj a :cljs b)` and its splicing form `#?@(:clj [a b] ...)`.
    ///
    /// Branches that do not match are still *read* (they have to parse), then
    /// dropped. No matching branch yields nothing at all, which is what Clojure
    /// does too.
    fn read_conditional(&mut self, start: usize) -> Result<Item, ReadError> {
        self.bump();
        let splicing = self.peek() == Some('@');
        if splicing {
            self.bump();
        }
        self.skip_ignorable();
        if self.peek() != Some('(') {
            return Err(ReadError::new(
                start,
                "a reader conditional must be followed by a list of feature/form pairs",
            ));
        }
        let list_at = self.pos;
        self.bump();
        let items = self.read_collection(list_at, ')')?;

        if items.len() % 2 != 0 {
            return Err(ReadError::new(
                start,
                "a reader conditional needs an even number of feature/form pairs",
            ));
        }

        let mut selected = None;
        for pair in items.chunks_exact(2) {
            let feature = pair[0].keyword().ok_or_else(|| {
                ReadError::new(
                    pair[0].offset,
                    format!(
                        "a reader conditional feature must be a keyword, found {}",
                        pair[0].describe()
                    ),
                )
            })?;
            if feature == FEATURE || feature == FEATURE_DEFAULT {
                selected = Some(pair[1].clone());
                break;
            }
        }

        let Some(selected) = selected else {
            return Ok(Item::Nothing);
        };

        if !splicing {
            return Ok(Item::One(selected));
        }

        match selected.kind {
            Kind::Vector(forms) | Kind::List(forms) => Ok(Item::Splice(forms)),
            _ => Err(ReadError::new(
                selected.offset,
                format!(
                    "a splicing reader conditional needs a sequence to splice, found {}",
                    selected.describe()
                ),
            )),
        }
    }

    fn read_string(&mut self) -> Result<String, ReadError> {
        let open_at = self.pos;
        self.bump();
        let mut out = String::new();
        loop {
            let Some(next) = self.bump() else {
                return Err(ReadError::new(open_at, "unterminated string"));
            };
            match next {
                '"' => return Ok(out),
                '\\' => out.push(self.read_string_escape()?),
                _ => out.push(next),
            }
        }
    }

    fn read_string_escape(&mut self) -> Result<char, ReadError> {
        let escape_at = self.pos;
        let Some(next) = self.bump() else {
            return Err(ReadError::new(escape_at, "unterminated escape sequence"));
        };
        match next {
            't' => Ok('\t'),
            'r' => Ok('\r'),
            'n' => Ok('\n'),
            'b' => Ok('\u{8}'),
            'f' => Ok('\u{c}'),
            '\\' => Ok('\\'),
            '"' => Ok('"'),
            'u' => self.read_unicode_escape(escape_at),
            '0'..='7' => self.read_octal_escape(escape_at, next),
            _ => Err(ReadError::new(
                escape_at,
                format!("unsupported escape sequence `\\{next}`"),
            )),
        }
    }

    fn read_unicode_escape(&mut self, escape_at: usize) -> Result<char, ReadError> {
        let mut digits = String::with_capacity(4);
        for _ in 0..4 {
            let Some(next) = self.bump() else {
                return Err(ReadError::new(escape_at, "truncated `\\u` escape"));
            };
            digits.push(next);
        }
        let code = u32::from_str_radix(&digits, 16)
            .map_err(|_| ReadError::new(escape_at, format!("`\\u{digits}` is not hexadecimal")))?;
        char::from_u32(code)
            .ok_or_else(|| ReadError::new(escape_at, format!("`\\u{digits}` is not a character")))
    }

    fn read_octal_escape(&mut self, escape_at: usize, first: char) -> Result<char, ReadError> {
        let mut digits = String::with_capacity(3);
        digits.push(first);
        while digits.len() < 3 {
            match self.peek() {
                Some(next @ '0'..='7') => {
                    self.bump();
                    digits.push(next);
                }
                _ => break,
            }
        }
        let code = u32::from_str_radix(&digits, 8)
            .map_err(|_| ReadError::new(escape_at, format!("`\\{digits}` is not octal")))?;
        char::from_u32(code)
            .ok_or_else(|| ReadError::new(escape_at, format!("`\\{digits}` is not a character")))
    }

    /// `#"..."`. Unlike a string, a backslash keeps its following character
    /// verbatim for the regex engine; it only stops `"` from closing the
    /// literal.
    fn read_regex(&mut self) -> Result<(), ReadError> {
        let open_at = self.pos;
        self.bump();
        loop {
            let Some(next) = self.bump() else {
                return Err(ReadError::new(open_at, "unterminated regex literal"));
            };
            if next == '"' {
                return Ok(());
            }
            if next == '\\' && self.bump().is_none() {
                return Err(ReadError::new(open_at, "unterminated regex literal"));
            }
        }
    }

    /// `\a`, `\newline`, `é`, `\(`. The first character is taken
    /// unconditionally so that punctuation characters read correctly; a named
    /// character then continues as a token.
    fn read_character(&mut self) -> Result<(), ReadError> {
        let start = self.pos;
        self.bump();
        let Some(first) = self.bump() else {
            return Err(ReadError::new(start, "end of input after `\\`"));
        };
        if is_token_char(first) {
            self.read_token();
        }
        Ok(())
    }

    /// The leading `:` is dropped, so an auto-resolved `::require` keeps a
    /// second colon and never compares equal to a plain `:require`.
    fn read_keyword(&mut self) -> String {
        self.bump();
        self.read_token()
    }

    fn read_token(&mut self) -> String {
        let start = self.pos;
        while let Some(next) = self.peek() {
            if is_token_char(next) {
                self.bump();
            } else {
                break;
            }
        }
        self.src[start..self.pos].to_owned()
    }

    fn read_token_form(&mut self, start: usize) -> Result<Kind, ReadError> {
        let numeric = self.starts_number();
        let text = self.read_token();
        if text.is_empty() {
            let found = self.peek().unwrap_or('\0');
            return Err(ReadError::new(
                start,
                format!("unexpected character `{found}`"),
            ));
        }
        if numeric {
            return Ok(Kind::Atom);
        }
        Ok(Kind::Symbol(text))
    }

    /// A token is a number when it opens with a digit, or with a sign or dot
    /// immediately followed by one. Anything else is a symbol.
    fn starts_number(&self) -> bool {
        match self.peek() {
            Some(next) if next.is_ascii_digit() => true,
            Some('+' | '-' | '.') => self.peek_second().is_some_and(|next| next.is_ascii_digit()),
            _ => false,
        }
    }
}

/// Clojure's *terminating* macro characters end a token; `#`, `'` and `%` do
/// not, which is why `foo'` and `foo#` are single symbols.
const fn is_token_char(candidate: char) -> bool {
    !candidate.is_whitespace()
        && !matches!(
            candidate,
            '(' | ')' | '[' | ']' | '{' | '}' | '"' | ';' | ',' | '`' | '~' | '^' | '@' | '\\'
        )
}

#[cfg(test)]
mod tests {
    use super::{Form, Kind, Reader};

    fn read_all(src: &str) -> Vec<Form> {
        let mut reader = Reader::new(src);
        let mut forms = Vec::new();
        while let Some(form) = reader.next_form().expect("source should read") {
            forms.push(form);
        }
        forms
    }

    fn kinds(src: &str) -> Vec<Kind> {
        read_all(src).into_iter().map(|form| form.kind).collect()
    }

    fn symbols(kind: &Kind) -> Vec<String> {
        let (Kind::List(items) | Kind::Vector(items) | Kind::Set(items) | Kind::Map(items)) = kind
        else {
            return Vec::new();
        };
        items
            .iter()
            .filter_map(|item| item.symbol().map(str::to_owned))
            .collect()
    }

    #[test]
    fn reads_nested_collections() {
        let kinds = kinds("(a [b {c #{d}}])");
        assert_eq!(kinds.len(), 1);
        let Kind::List(outer) = &kinds[0] else {
            panic!("expected a list, got {:?}", kinds[0]);
        };
        assert_eq!(outer[0].symbol(), Some("a"));
        assert!(matches!(outer[1].kind, Kind::Vector(_)));
    }

    #[test]
    fn commas_and_line_comments_are_whitespace() {
        let kinds = kinds("(a, ;; a comment with ) and \"quotes\"\n b)");
        assert_eq!(symbols(&kinds[0]), ["a", "b"]);
    }

    #[test]
    fn discard_drops_the_next_form() {
        let kinds = kinds("(a #_ [ignored me] b)");
        assert_eq!(symbols(&kinds[0]), ["a", "b"]);
    }

    #[test]
    fn metadata_is_dropped_and_its_target_kept() {
        let kinds = kinds("(^{:doc \"x\"} a ^:private b)");
        assert_eq!(symbols(&kinds[0]), ["a", "b"]);
    }

    #[test]
    fn reader_conditional_selects_the_clj_branch() {
        let kinds = kinds("(a #?(:cljs cljs-only :clj clj-only) b)");
        assert_eq!(symbols(&kinds[0]), ["a", "clj-only", "b"]);
    }

    #[test]
    fn reader_conditional_falls_back_to_default() {
        let kinds = kinds("(a #?(:cljs cljs-only :default shared) b)");
        assert_eq!(symbols(&kinds[0]), ["a", "shared", "b"]);
    }

    #[test]
    fn reader_conditional_with_no_match_yields_nothing() {
        let kinds = kinds("(a #?(:cljs cljs-only) b)");
        assert_eq!(symbols(&kinds[0]), ["a", "b"]);
    }

    #[test]
    fn splicing_reader_conditional_flattens_into_the_parent() {
        let kinds = kinds("(a #?@(:clj [x y] :cljs [z]) b)");
        assert_eq!(symbols(&kinds[0]), ["a", "x", "y", "b"]);
    }

    #[test]
    fn strings_regexes_and_characters_do_not_break_delimiter_tracking() {
        let kinds = kinds(r#"(a "a ) string \" here" #"re\"gex)" \( \newline b)"#);
        assert_eq!(symbols(&kinds[0]), ["a", "b"]);
    }

    #[test]
    fn numbers_are_atoms_and_signed_symbols_are_not() {
        assert_eq!(kinds("42 -1.5 3/4"), [Kind::Atom, Kind::Atom, Kind::Atom]);
        assert_eq!(
            kinds("-> +foo"),
            [Kind::Symbol("->".into()), Kind::Symbol("+foo".into())]
        );
    }

    #[test]
    fn auto_resolved_keywords_stay_distinct_from_plain_ones() {
        assert_eq!(
            kinds(":require ::require"),
            [
                Kind::Keyword("require".into()),
                Kind::Keyword(":require".into())
            ]
        );
    }

    #[test]
    fn tagged_literals_yield_their_value() {
        let kinds = kinds("(a #inst \"2024-01-01\" b)");
        assert_eq!(symbols(&kinds[0]), ["a", "b"]);
    }

    #[test]
    fn a_mismatched_delimiter_names_both_ends() {
        let error = Reader::new("(a [b c)").next_form().expect_err("should fail");
        assert_eq!(error.offset, 7);
        assert!(
            error.message.contains("found `)` where `]` was expected"),
            "{}",
            error.message
        );
        assert!(error.message.contains("offset 3"), "{}", error.message);
    }

    #[test]
    fn an_unclosed_form_reports_where_it_opened() {
        let error = Reader::new("(a [b c").next_form().expect_err("should fail");
        assert_eq!(error.offset, 3);
        assert!(error.message.contains("unclosed"), "{}", error.message);
    }

    #[test]
    fn unterminated_string_reports_its_opening_offset() {
        let error = Reader::new("(a \"oops)").next_form().expect_err("should fail");
        assert_eq!(error.offset, 3);
    }
}
