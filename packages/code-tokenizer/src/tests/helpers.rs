use crate::CodeTokenizer;
use tantivy::tokenizer::{LowerCaser, Stemmer, TextAnalyzer, TokenStream, Tokenizer};

pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokenizer = CodeTokenizer;
    let mut stream = tokenizer.token_stream(text);
    let mut tokens = Vec::new();
    while stream.advance() {
        tokens.push(stream.token().text.clone());
    }
    tokens
}

/// One emitted token's text and byte span, so tests can pin the offsets that
/// index queries and result highlighting rely on, not just the lowered text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSpan {
    /// The lowered token text.
    pub text: String,
    /// The token's ordinal position in the stream.
    pub position: usize,
    /// Byte offset where the token starts in the source.
    pub offset_from: usize,
    /// Byte offset just past the token's end in the source.
    pub offset_to: usize,
}

/// Collect a [`TokenSpan`] for every emitted token.
pub fn tokenize_full(text: &str) -> Vec<TokenSpan> {
    let mut tokenizer = CodeTokenizer;
    let mut stream = tokenizer.token_stream(text);
    let mut tokens = Vec::new();
    while stream.advance() {
        let token = stream.token();
        tokens.push(TokenSpan {
            text: token.text.clone(),
            position: token.position,
            offset_from: token.offset_from,
            offset_to: token.offset_to,
        });
    }
    tokens
}

pub fn tokenize_stemmed(text: &str) -> Vec<String> {
    let mut analyzer = TextAnalyzer::builder(CodeTokenizer)
        .filter(LowerCaser)
        .filter(Stemmer::default())
        .build();
    let mut stream = analyzer.token_stream(text);
    let mut tokens = Vec::new();
    while stream.advance() {
        tokens.push(stream.token().text.clone());
    }
    tokens
}
