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

/// `(text, position, offset_from, offset_to)` for every emitted token, so
/// tests can pin the byte spans that index queries and result highlighting
/// rely on, not just the lowered token text.
pub fn tokenize_full(text: &str) -> Vec<(String, usize, usize, usize)> {
    let mut tokenizer = CodeTokenizer;
    let mut stream = tokenizer.token_stream(text);
    let mut tokens = Vec::new();
    while stream.advance() {
        let token = stream.token();
        tokens.push((
            token.text.clone(),
            token.position,
            token.offset_from,
            token.offset_to,
        ));
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
