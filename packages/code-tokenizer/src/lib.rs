//! Tantivy tokenizer that splits identifiers the way code reviewers read them.
//!
//! Tokens split on camelCase, snake_case, kebab-case, and any non-alphanumeric
//! run. ANSI escape sequences are stripped up front so terminal captures
//! tokenize the same way as their decoded contents.
//!
//! Two analyzers are registered on the index:
//!
//! - [`CODE_TOKENIZER`] — the raw split, useful for facet-style exact lookup.
//! - [`CODE_STEMMED_TOKENIZER`] — same split plus lowercasing and English
//!   stemming, used for ranked full-text search.

use tantivy::tokenizer::{LowerCaser, Stemmer, TextAnalyzer, Token, TokenStream, Tokenizer};

pub const CODE_TOKENIZER: &str = "code";
pub const CODE_STEMMED_TOKENIZER: &str = "code_stemmed";

pub fn register_tokenizers(index: &tantivy::Index) {
    let code_tokenizer = CodeTokenizer;
    index
        .tokenizers()
        .register(CODE_TOKENIZER, code_tokenizer.clone());

    let code_stemmed = TextAnalyzer::builder(code_tokenizer)
        .filter(LowerCaser)
        .filter(Stemmer::default())
        .build();
    index
        .tokenizers()
        .register(CODE_STEMMED_TOKENIZER, code_stemmed);
}

#[derive(Clone)]
pub struct CodeTokenizer;

impl Tokenizer for CodeTokenizer {
    type TokenStream<'a> = CodeTokenStream;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> Self::TokenStream<'a> {
        let stripped = strip_ansi_escapes::strip(text.as_bytes());
        let text_without_escapes = String::from_utf8_lossy(&stripped).into_owned();

        CodeTokenStream::new(text_without_escapes)
    }
}

pub struct CodeTokenStream {
    text: String,
    position: usize,
    token: Token,
    byte_offset: usize,
}

impl CodeTokenStream {
    fn new(text: String) -> Self {
        Self {
            text,
            position: 0,
            token: Token::default(),
            byte_offset: 0,
        }
    }

    fn is_separator(ch: char) -> bool {
        ch == '_' || ch == '-'
    }

    fn should_break_on_case_change(ch: char, last_was_lower_or_digit: bool) -> bool {
        ch.is_uppercase() && last_was_lower_or_digit
    }

    fn should_stop_consuming(ch: char, last_was_lower_or_digit: bool) -> bool {
        Self::is_separator(ch)
            || Self::should_break_on_case_change(ch, last_was_lower_or_digit)
            || !ch.is_alphanumeric()
    }
}

impl TokenStream for CodeTokenStream {
    fn advance(&mut self) -> bool {
        self.token.text.clear();
        self.token.position = self.position;

        if self.byte_offset >= self.text.len() {
            return false;
        }

        let Some(remaining) = self.text.get(self.byte_offset..) else {
            return false;
        };
        let mut current_offset = self.byte_offset;
        let mut chars = remaining.chars();

        let start_offset = loop {
            let Some(ch) = chars.next() else {
                return false;
            };

            let char_start = current_offset;
            current_offset += ch.len_utf8();

            if ch.is_alphanumeric() {
                self.token.text.push(ch.to_ascii_lowercase());
                break char_start;
            }
        };

        self.token.offset_from = start_offset;

        let mut last_was_lower_or_digit = false;

        loop {
            let Some(ch) = chars.clone().next() else {
                self.token.offset_to = current_offset;
                self.byte_offset = current_offset;
                self.position += 1;
                return true;
            };

            if Self::should_stop_consuming(ch, last_was_lower_or_digit) {
                if Self::is_separator(ch) {
                    current_offset += ch.len_utf8();
                    chars.next();
                }
                self.token.offset_to = current_offset;
                self.byte_offset = current_offset;
                self.position += 1;
                return true;
            }

            chars.next();
            current_offset += ch.len_utf8();
            self.token.text.push(ch.to_ascii_lowercase());
            last_was_lower_or_digit = ch.is_lowercase() || ch.is_numeric();
        }
    }

    fn token(&self) -> &Token {
        &self.token
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.token
    }
}

#[cfg(test)]
mod tests;
