//! Map boundary types and literal defaults to Python source text.

use unibind_core::ir;

/// Where an annotation appears. Paths are the position-sensitive case: an
/// argument accepts `str | os.PathLike[str]` (what the generated `pyo3`
/// wrapper extracts a `PathBuf` from), while a returned or stored path is
/// always rendered back as `str`.
#[derive(Clone, Copy)]
pub enum Position {
    /// A function or constructor argument.
    Argument,
    /// A return type or record field.
    Return,
}

/// The Python annotation for `ty` at `position`. Named types resolve to the
/// record's or object's Python name through `interface`.
pub fn annotation(interface: &ir::Interface, ty: &ir::Type, position: Position) -> String {
    match ty {
        ir::Type::Bool => "bool".to_owned(),
        ir::Type::Int(_) => "int".to_owned(),
        ir::Type::Float(_) => "float".to_owned(),
        ir::Type::String { .. } => "str".to_owned(),
        ir::Type::Path { .. } => match position {
            Position::Argument => "str | os.PathLike[str]".to_owned(),
            Position::Return => "str".to_owned(),
        },
        ir::Type::Bytes { .. } => "bytes".to_owned(),
        ir::Type::Option(inner) => format!("{} | None", annotation(interface, inner, position)),
        ir::Type::Vec(inner) => format!("list[{}]", annotation(interface, inner, position)),
        ir::Type::Map { key, value } => format!(
            "dict[{}, {}]",
            annotation(interface, key, position),
            annotation(interface, value, position)
        ),
        ir::Type::Named(name) => named_py_name(interface, name),
        // Stream items are always produced, never accepted, so the item
        // renders in return position regardless of where the stream sits.
        ir::Type::Stream(item) => format!(
            "collections.abc.AsyncIterator[{}]",
            annotation(interface, item, Position::Return)
        ),
    }
}

/// Whether `ty` mentions a filesystem path anywhere; a stub that renders one
/// in argument position needs `import os` for the `os.PathLike` form.
pub fn mentions_path(ty: &ir::Type) -> bool {
    match ty {
        ir::Type::Path { .. } => true,
        ir::Type::Option(inner) | ir::Type::Vec(inner) | ir::Type::Stream(inner) => {
            mentions_path(inner)
        }
        ir::Type::Map { key, value } => mentions_path(key) || mentions_path(value),
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Bytes { .. }
        | ir::Type::Named(_) => false,
    }
}

/// Render a literal default as Python source.
pub fn literal(value: &ir::Literal) -> String {
    match value {
        ir::Literal::Bool(true) => "True".to_owned(),
        ir::Literal::Bool(false) => "False".to_owned(),
        ir::Literal::Int(int) => int.to_string(),
        // `{:?}` keeps a fractional part (`1.0`, not `1`), so the rendered
        // default stays a Python float literal.
        ir::Literal::Float(float) => format!("{float:?}"),
        ir::Literal::Str(text) => str_literal(text),
        ir::Literal::None => "None".to_owned(),
    }
}

/// The Python name of an interface item: the `py` override when set, the
/// Rust name otherwise. Same rule the `pyo3` backend applies.
pub fn py_name<'a>(names: &'a ir::Names, name: &'a str) -> &'a str {
    names.py.as_deref().unwrap_or(name)
}

/// Resolve a `Named` reference (a record or an object) to its Python name.
fn named_py_name(interface: &ir::Interface, name: &str) -> String {
    if let Some(record) = interface.records.iter().find(|record| record.name == name) {
        return py_name(&record.names, &record.name).to_owned();
    }
    interface
        .objects
        .iter()
        .find(|object| object.name == name)
        .map_or(name, |object| py_name(&object.names, &object.name))
        .to_owned()
}

/// A double-quoted Python string literal.
fn str_literal(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
