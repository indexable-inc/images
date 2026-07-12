//! Map IR types onto Elixir typespecs and defaults onto Elixir literals.

use unibind_core::ir;

use crate::names;

/// The Elixir typespec of a boundary type. `ns` qualifies records, whose
/// structs live under the namespace module; `interface` resolves their
/// Elixir names.
pub fn typespec(ty: &ir::Type, interface: &ir::Interface, ns: &str) -> String {
    match ty {
        ir::Type::Bool => "boolean()".to_owned(),
        ir::Type::Int(_) => "integer()".to_owned(),
        ir::Type::Float(_) => "float()".to_owned(),
        ir::Type::String { .. } | ir::Type::Path { .. } => "String.t()".to_owned(),
        // Rejected before any spec is rendered; spelled for completeness.
        ir::Type::Bytes { .. } => "binary()".to_owned(),
        ir::Type::Option(inner) => format!("{} | nil", typespec(inner, interface, ns)),
        ir::Type::Vec(inner) => format!("[{}]", typespec(inner, interface, ns)),
        ir::Type::Map { key, value } => format!(
            "%{{optional({}) => {}}}",
            typespec(key, interface, ns),
            typespec(value, interface, ns)
        ),
        ir::Type::Named(name) => {
            format!("{ns}.{}.t()", names::ex_record_name_of(interface, name))
        }
        ir::Type::Stream(_) => "Enumerable.t()".to_owned(),
    }
}

/// The Elixir literal of a default value.
pub fn literal(literal: &ir::Literal) -> String {
    match literal {
        ir::Literal::Bool(value) => value.to_string(),
        ir::Literal::Int(value) => value.to_string(),
        ir::Literal::Float(value) => {
            let text = value.to_string();
            // Elixir float literals need a decimal point (`1.0`, not `1`).
            if text.contains(['.', 'e', 'E']) {
                text
            } else {
                format!("{text}.0")
            }
        }
        ir::Literal::Str(value) => quote_string(value),
        ir::Literal::None => "nil".to_owned(),
    }
}

/// An Elixir double-quoted string literal.
pub fn quote_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '#' => out.push_str("\\#"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
