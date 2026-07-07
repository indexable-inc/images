//! Name mapping between the IR's Rust names and Swift spellings.

use proc_macro2::Ident;

use crate::RenderError;

/// An identifier for a Rust-side name coming out of the IR.
pub fn name_ident(name: &str) -> Result<Ident, RenderError> {
    syn::parse_str::<Ident>(name)
        .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
        .map_err(|_| RenderError::new(format!("`{name}` is not usable as an identifier")))
}

/// A Rust name as Swift lowerCamelCase: `touch_path` becomes `touchPath`,
/// and a PascalCase variant name like `StoreGone` becomes `storeGone`.
pub fn lower_camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper_next = false;
    for character in name.chars() {
        if character == '_' {
            upper_next = !out.is_empty();
        } else if upper_next {
            out.extend(character.to_uppercase());
            upper_next = false;
        } else if out.is_empty() {
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// A snake_case name from a camel-case mangle (`VecOfString` becomes
/// `vec_of_string`), for the generated constructor function names.
pub fn to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for character in name.chars() {
        if character.is_uppercase() {
            if !out.is_empty() {
                out.push('_');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// Escape a Swift keyword with backticks so a Rust name like `default` can
/// still appear as a Swift argument or property name.
pub fn swift_name(name: &str) -> String {
    let camel = lower_camel(name);
    if SWIFT_KEYWORDS.contains(&camel.as_str()) {
        return format!("`{camel}`");
    }
    camel
}

/// Swift keywords that need backticks in declaration position (the subset
/// that can plausibly collide with Rust item names).
const SWIFT_KEYWORDS: &[&str] = &[
    "as", "break", "case", "catch", "class", "continue", "default", "defer", "do", "else", "enum",
    "extension", "false", "for", "func", "guard", "if", "import", "in", "init", "inout", "is",
    "let", "nil", "operator", "private", "protocol", "public", "repeat", "return", "self",
    "static", "struct", "subscript", "switch", "throw", "throws", "true", "try", "typealias",
    "var", "where", "while",
];
