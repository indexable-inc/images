use super::helpers::{tokenize, tokenize_stemmed};

#[test]
fn ansi_escape_codes_stripped() {
    assert_eq!(
        tokenize("\x1b[32mhello\x1b[0m_world"),
        vec!["hello", "world"]
    );
}

#[test]
fn multiple_ansi_codes_stripped() {
    assert_eq!(
        tokenize("\x1b[1;31mget\x1b[0m\x1b[32mUser\x1b[0mById"),
        vec!["get", "user", "by", "id"]
    );
}

#[test]
fn ansi_codes_with_stemming() {
    assert_eq!(
        tokenize_stemmed("\x1b[33mrunning\x1b[0m_tests"),
        vec!["run", "test"]
    );
}
