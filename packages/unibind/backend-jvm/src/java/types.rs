//! Java spellings, `ValueLayout` names, literals, and doc plumbing for
//! boundary types.

use std::fmt::Write as _;

use unibind_core::ir;

use crate::ctype::CTy;
use crate::RenderError;

/// The Java type at one boundary position; `boxed` for generic slots and
/// optional values.
pub fn java_type(ty: &CTy, boxed: bool) -> String {
    match ty {
        CTy::Bool => scalar_name("boolean", "Boolean", boxed),
        CTy::Int(kind) => int_type(*kind, boxed),
        CTy::Float(ir::FloatKind::F32) => scalar_name("float", "Float", boxed),
        CTy::Float(ir::FloatKind::F64) => scalar_name("double", "Double", boxed),
        CTy::Str => "String".to_owned(),
        CTy::Path => "java.nio.file.Path".to_owned(),
        CTy::Bytes => "byte[]".to_owned(),
        CTy::Option(inner) => java_type(inner, true),
        CTy::Vec(inner) => format!("java.util.List<{}>", java_type(inner, true)),
        CTy::Map { key, value } => format!(
            "java.util.Map<{}, {}>",
            java_type(key, true),
            java_type(value, true)
        ),
        CTy::Record(name) => name.clone(),
    }
}

fn scalar_name(primitive: &str, boxed_name: &str, boxed: bool) -> String {
    if boxed { boxed_name } else { primitive }.to_owned()
}

fn int_type(kind: ir::IntKind, boxed: bool) -> String {
    match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => scalar_name("byte", "Byte", boxed),
        ir::IntKind::I16 | ir::IntKind::U16 => scalar_name("short", "Short", boxed),
        ir::IntKind::I32 | ir::IntKind::U32 => scalar_name("int", "Integer", boxed),
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize => {
            scalar_name("long", "Long", boxed)
        }
    }
}

/// The `ValueLayout` constant carrying one mirror in a downcall descriptor:
/// scalars by width, aggregates as pointers.
pub const fn value_layout(ty: &CTy) -> &'static str {
    match ty {
        CTy::Bool => "ValueLayout.JAVA_BYTE",
        CTy::Int(kind) => int_layout_name(*kind),
        CTy::Float(ir::FloatKind::F32) => "ValueLayout.JAVA_FLOAT",
        CTy::Float(ir::FloatKind::F64) => "ValueLayout.JAVA_DOUBLE",
        CTy::Str
        | CTy::Path
        | CTy::Bytes
        | CTy::Option(_)
        | CTy::Vec(_)
        | CTy::Map { .. }
        | CTy::Record(_) => "ValueLayout.ADDRESS",
    }
}

const fn int_layout_name(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => "ValueLayout.JAVA_BYTE",
        ir::IntKind::I16 | ir::IntKind::U16 => "ValueLayout.JAVA_SHORT",
        ir::IntKind::I32 | ir::IntKind::U32 => "ValueLayout.JAVA_INT",
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize => {
            "ValueLayout.JAVA_LONG"
        }
    }
}

/// The Java expression reading a scalar mirror at `offset` inside `seg`.
pub fn scalar_get(ty: &CTy, seg: &str, offset: &str) -> String {
    let layout = value_layout(ty);
    let get = format!("{seg}.get({layout}, {offset})");
    if matches!(ty, CTy::Bool) {
        format!("{get} != 0")
    } else {
        get
    }
}

/// The Java statement (no trailing `;`) writing scalar `value` at `offset`
/// inside `seg`.
pub fn scalar_set(ty: &CTy, seg: &str, offset: &str, value: &str) -> String {
    let layout = value_layout(ty);
    let value = if matches!(ty, CTy::Bool) {
        format!("(byte) ({value} ? 1 : 0)")
    } else {
        value.to_owned()
    };
    format!("{seg}.set({layout}, {offset}, {value})")
}

/// The downcall argument expression for a scalar Java parameter.
pub fn downcall_arg(ty: &CTy, expr: &str) -> String {
    if matches!(ty, CTy::Bool) {
        format!("(byte) ({expr} ? 1 : 0)")
    } else {
        expr.to_owned()
    }
}

/// `offset` plus a constant, folding away `+ 0`.
pub fn offset_expr(base: &str, delta: u64) -> String {
    if delta == 0 {
        base.to_owned()
    } else {
        format!("{base} + {delta}")
    }
}

/// Doc notes for spellings that lose information in Java.
pub fn doc_notes(ty: &CTy) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if let CTy::Option(inner) = ty {
        notes.push("May be null.");
        notes.extend(doc_notes(inner));
        return notes;
    }
    if matches!(
        ty,
        CTy::Int(
            ir::IntKind::U8
                | ir::IntKind::U16
                | ir::IntKind::U32
                | ir::IntKind::U64
                | ir::IntKind::Usize
        )
    ) {
        notes.push("Unsigned in Rust; a negative value is the raw two's-complement bit pattern.");
    }
    notes
}

/// Render a doc-comment block (javadoc and `KDoc` share the syntax) at
/// `indent` levels of four spaces.
pub fn doc_block(lines: &[String], indent: usize) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let pad = "    ".repeat(indent);
    let mut out = format!("{pad}/**\n");
    for line in lines {
        if line.is_empty() {
            let _ = writeln!(out, "{pad} *");
        } else {
            let _ = writeln!(out, "{pad} * {line}");
        }
    }
    let _ = writeln!(out, "{pad} */");
    out
}

/// The Java expression for a default literal at type `ty`.
pub fn java_literal(ty: &CTy, literal: &ir::Literal) -> Result<String, RenderError> {
    match (ty, literal) {
        (CTy::Option(_), ir::Literal::None) => Ok("null".to_owned()),
        (CTy::Option(inner), _) => java_literal(inner, literal),
        (CTy::Bool, ir::Literal::Bool(value)) => Ok(value.to_string()),
        (CTy::Int(kind), ir::Literal::Int(value)) => Ok(int_literal(*kind, *value)),
        (CTy::Float(ir::FloatKind::F32), ir::Literal::Int(value)) => Ok(format!("{value}.0f")),
        (CTy::Float(ir::FloatKind::F64), ir::Literal::Int(value)) => Ok(format!("{value}.0")),
        (CTy::Float(ir::FloatKind::F32), ir::Literal::Float(value)) => Ok(format!("{value:?}f")),
        (CTy::Float(ir::FloatKind::F64), ir::Literal::Float(value)) => Ok(format!("{value:?}")),
        (CTy::Str, ir::Literal::Str(value)) => Ok(quoted(value, false)),
        (ty, literal) => Err(RenderError::new(format!(
            "default `{literal:?}` does not fit the `{}` parameter type",
            java_type(ty, false)
        ))),
    }
}

fn int_literal(kind: ir::IntKind, value: i64) -> String {
    match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => format!("(byte) {value}"),
        ir::IntKind::I16 | ir::IntKind::U16 => format!("(short) {value}"),
        ir::IntKind::I32 | ir::IntKind::U32 => value.to_string(),
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize => {
            format!("{value}L")
        }
    }
}

/// A double-quoted Java or Kotlin string literal; Kotlin additionally needs
/// `$` escaped out of template position.
pub fn quoted(value: &str, escape_dollar: bool) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '$' if escape_dollar => out.push_str("\\$"),
            other if (other as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", other as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}
