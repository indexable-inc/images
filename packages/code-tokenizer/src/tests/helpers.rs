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
