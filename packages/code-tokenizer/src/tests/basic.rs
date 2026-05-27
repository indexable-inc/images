use super::helpers::tokenize;

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
