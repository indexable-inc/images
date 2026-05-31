pub const IDENTIFIERS: &[&str] = &[
    "identifier",
    "type_identifier",
    "field_identifier",
    "property_identifier",
    "shorthand_property_identifier",
    "self",
    "metavariable",
    "private_property_identifier",
    "attribute",
    "package_identifier",
    "simple_identifier",
];

pub const LITERALS: &[&str] = &[
    "integer_literal",
    "float_literal",
    "number",
    "decimal_integer_literal",
    "hex_integer_literal",
    "octal_integer_literal",
    "binary_integer_literal",
    "string_literal",
    "string",
    "string_fragment",
    "string_content",
    "raw_string_literal",
    "char_literal",
    "template_string",
    "true",
    "false",
    "boolean",
    "null",
    "nil",
    "none",
];

pub const SIGNIFICANT: &[&str] = &[
    // Functions/methods
    "function_item",
    "function_definition",
    "function_declaration",
    "method_definition",
    "method_declaration",
    "arrow_function",
    "lambda_expression",
    "closure_expression",
    // Types/structures
    "class_declaration",
    "class_definition",
    "struct_item",
    "impl_item",
    "trait_item",
    "interface_declaration",
    "enum_item",
    "enum_declaration",
    // Blocks
    "block",
    "statement_block",
    "compound_statement",
    // Control flow (enables sub-block duplicate detection)
    "if_expression",
    "if_statement",
    "if_let_expression",
    "for_expression",
    "for_statement",
    "for_in_statement",
    "while_expression",
    "while_statement",
    "match_expression",
    "switch_statement",
    "match_arm",
    "switch_case",
    "try_expression",
    "try_statement",
];

#[must_use]
pub fn is_normalizable(kind: &str) -> bool {
    IDENTIFIERS.contains(&kind) || LITERALS.contains(&kind)
}

#[must_use]
pub fn is_identifier(kind: &str) -> bool {
    IDENTIFIERS.contains(&kind)
}

#[must_use]
pub fn is_significant(kind: &str) -> bool {
    SIGNIFICANT.contains(&kind)
}
