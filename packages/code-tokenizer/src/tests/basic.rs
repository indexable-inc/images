use super::helpers::{tokenize, tokenize_full};

#[test]
fn camel_case() {
    assert_eq!(tokenize("HelloWorld"), vec!["hello", "world"]);
}

#[test]
fn snake_case() {
    assert_eq!(tokenize("hello_world"), vec!["hello", "world"]);
}

#[test]
fn mixed() {
    assert_eq!(tokenize("helloWorld"), vec!["hello", "world"]);
}

#[test]
fn uppercase() {
    assert_eq!(tokenize("HELLO_WORLD"), vec!["hello", "world"]);
}

#[test]
fn camel_case_this_is_a_test() {
    assert_eq!(tokenize("thisIsATest"), vec!["this", "is", "a", "test"]);
}

#[test]
fn camel_case_complex() {
    assert_eq!(tokenize("getUserById"), vec!["get", "user", "by", "id"]);
}

#[test]
fn snake_case_complex() {
    assert_eq!(tokenize("get_user_by_id"), vec!["get", "user", "by", "id"]);
}

#[test]
fn kebab_case() {
    assert_eq!(tokenize("get-user-by-id"), vec!["get", "user", "by", "id"]);
}

#[test]
fn mixed_snake_camel() {
    assert_eq!(tokenize("get_userById"), vec!["get", "user", "by", "id"]);
}

#[test]
fn acronyms() {
    // Acronym followed by camelCase splits before the last uppercase so
    // queries for either `http` or `server` match the symbol.
    assert_eq!(tokenize("HTTPServer"), vec!["http", "server"]);
}

#[test]
fn single_letter_word_in_middle() {
    // `getXValue` and friends were folded as `get` + `xvalue` before;
    // they now split cleanly so queries for the suffix match.
    assert_eq!(tokenize("getXValue"), vec!["get", "x", "value"]);
    assert_eq!(tokenize("isAWidget"), vec!["is", "a", "widget"]);
}

#[test]
fn numbers() {
    assert_eq!(tokenize("user123Id"), vec!["user123", "id"]);
}

#[test]
fn special_chars_ignored() {
    assert_eq!(tokenize("hello@world!test"), vec!["hello", "world", "test"]);
}

#[test]
fn leading_trailing_special() {
    assert_eq!(tokenize("!!!hello___world!!!"), vec!["hello", "world"]);
}

#[test]
fn empty_string() {
    assert_eq!(tokenize(""), Vec::<String>::new());
}

#[test]
fn only_special_chars() {
    assert_eq!(tokenize("@#$%^&*()"), Vec::<String>::new());
}

#[test]
fn single_word() {
    assert_eq!(tokenize("hello"), vec!["hello"]);
}

#[test]
fn single_uppercase() {
    assert_eq!(tokenize("HELLO"), vec!["hello"]);
}

#[test]
fn single_letter_lowercase_prefix() {
    assert_eq!(tokenize("aTest"), vec!["a", "test"]);
}

#[test]
fn single_letter_prefix_long_word() {
    assert_eq!(tokenize("xCoordinate"), vec!["x", "coordinate"]);
}

#[test]
fn single_letter_prefix_etag() {
    assert_eq!(tokenize("eTag"), vec!["e", "tag"]);
}

#[test]
fn digit_prefix_splits_on_letter() {
    assert_eq!(tokenize("3DModel"), vec!["3", "d", "model"]);
}

#[test]
fn snake_case_token_shape() {
    assert_eq!(
        tokenize_full("hello_world"),
        vec![
            ("hello".to_string(), 0, 0, 5),
            ("world".to_string(), 1, 6, 11),
        ]
    );
}

#[test]
fn camel_case_token_shape() {
    assert_eq!(
        tokenize_full("helloWorld"),
        vec![
            ("hello".to_string(), 0, 0, 5),
            ("world".to_string(), 1, 5, 10),
        ]
    );
}

#[test]
fn kebab_case_token_shape_four_tokens() {
    assert_eq!(
        tokenize_full("get-user-by-id"),
        vec![
            ("get".to_string(), 0, 0, 3),
            ("user".to_string(), 1, 4, 8),
            ("by".to_string(), 2, 9, 11),
            ("id".to_string(), 3, 12, 14),
        ]
    );
}

#[test]
fn multi_byte_chars_keep_byte_offsets() {
    // `é` and `ö` are each two UTF-8 bytes, so the second token's
    // `offset_from` must skip both the multi-byte char inside the
    // first token and the single-byte separator.
    assert_eq!(
        tokenize_full("héllo_wörld"),
        vec![
            ("héllo".to_string(), 0, 0, 6),
            ("wörld".to_string(), 1, 7, 13),
        ]
    );
}

#[test]
fn single_token_eof_branch_shape() {
    assert_eq!(tokenize_full("hello"), vec![("hello".to_string(), 0, 0, 5)]);
}
