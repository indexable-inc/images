use crate::to_snake_case;

#[test]
fn camel_case() {
    assert_eq!(to_snake_case("HelloWorld"), "hello_world");
}

#[test]
fn lower_camel_case() {
    assert_eq!(to_snake_case("helloWorld"), "hello_world");
}

#[test]
fn kebab_case() {
    assert_eq!(to_snake_case("get-user-by-id"), "get_user_by_id");
}

#[test]
fn space_separated() {
    assert_eq!(to_snake_case("Hello World Name"), "hello_world_name");
}

#[test]
fn already_snake_case() {
    assert_eq!(to_snake_case("hello_world"), "hello_world");
}

#[test]
fn screaming_snake_case() {
    assert_eq!(to_snake_case("HELLO_WORLD"), "hello_world");
}

#[test]
fn acronym_run() {
    // Matches the tokenizer's acronym split so search and conversion agree.
    assert_eq!(to_snake_case("HTTPServer"), "http_server");
}

#[test]
fn single_letter_word() {
    assert_eq!(to_snake_case("getXValue"), "get_x_value");
}

#[test]
fn numbers_stay_attached() {
    assert_eq!(to_snake_case("user123Id"), "user123_id");
}

#[test]
fn mixed_delimiters_and_punctuation() {
    assert_eq!(to_snake_case("hello@world-fooBar"), "hello_world_foo_bar");
}

#[test]
fn collapses_repeated_separators() {
    assert_eq!(to_snake_case("  foo__bar  "), "foo_bar");
}

#[test]
fn single_word() {
    assert_eq!(to_snake_case("hello"), "hello");
}

#[test]
fn empty_string() {
    assert_eq!(to_snake_case(""), "");
}

#[test]
fn only_separators() {
    assert_eq!(to_snake_case("  -_-  "), "");
}
