use crate::kinds::{is_identifier, is_normalizable, is_significant};

#[test]
fn normalizable_identifiers() {
    assert!(is_normalizable("identifier"));
    assert!(is_normalizable("type_identifier"));
    assert!(is_normalizable("field_identifier"));
}

#[test]
fn normalizable_literals() {
    assert!(is_normalizable("integer_literal"));
    assert!(is_normalizable("string_literal"));
    assert!(is_normalizable("float_literal"));
}

#[test]
fn non_normalizable() {
    assert!(!is_normalizable("function_item"));
    assert!(!is_normalizable("let_declaration"));
    assert!(!is_normalizable("binary_expression"));
}

#[test]
fn identifier_check() {
    assert!(is_identifier("identifier"));
    assert!(is_identifier("type_identifier"));
    assert!(!is_identifier("integer_literal"));
    assert!(!is_identifier("string_literal"));
}

#[test]
fn significant_kind_check() {
    assert!(is_significant("function_item"));
    assert!(is_significant("function_definition"));
    assert!(is_significant("class_declaration"));
    assert!(is_significant("struct_item"));
    assert!(!is_significant("identifier"));
    assert!(!is_significant("binary_expression"));
}
