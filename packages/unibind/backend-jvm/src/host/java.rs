//! Java spellings: the type mapping, the wire-codec expression and
//! statement generators, and literal/javadoc escaping.

use std::fmt::Write as _;

use unibind_core::ir;

use crate::names;

/// The Java type of a declared position (a parameter, a return, a record
/// component). `Option<T>` is the boxed type of `T` with `null` for `None`;
/// Java's null model cannot spell the inner `None` of a nested option, so
/// [`crate::ty::check_boundary`] rejects those.
pub fn declared(ty: &ir::Type, interface: &ir::Interface) -> String {
    match ty {
        ir::Type::Bool => "boolean".to_owned(),
        ir::Type::Int(kind) => primitive_int(*kind).to_owned(),
        ir::Type::Float(ir::FloatKind::F32) => "float".to_owned(),
        ir::Type::Float(ir::FloatKind::F64) => "double".to_owned(),
        ir::Type::String { .. } => "String".to_owned(),
        ir::Type::Path { .. } => "Path".to_owned(),
        ir::Type::Bytes { .. } => "byte[]".to_owned(),
        ir::Type::Option(inner) => boxed(inner, interface),
        ir::Type::Vec(inner) => format!("List<{}>", boxed(inner, interface)),
        ir::Type::Map { key, value } => format!(
            "Map<{}, {}>",
            boxed(key, interface),
            boxed(value, interface)
        ),
        ir::Type::Named(name) => names::record_name_of(interface, name).to_owned(),
        ir::Type::Stream(_) => unreachable!("rejected by check_boundary"),
    }
}

/// The Java type of a generic position (a `List` element, a `Map` key or
/// value, a nullable option payload): [`declared`] with primitives boxed.
pub fn boxed(ty: &ir::Type, interface: &ir::Interface) -> String {
    match ty {
        ir::Type::Bool => "Boolean".to_owned(),
        ir::Type::Int(kind) => boxed_int(*kind).to_owned(),
        ir::Type::Float(ir::FloatKind::F32) => "Float".to_owned(),
        ir::Type::Float(ir::FloatKind::F64) => "Double".to_owned(),
        _ => declared(ty, interface),
    }
}

/// Unsigned kinds reinterpret as the signed Java type of the same width;
/// `isize`/`usize` travel as eight bytes and land as `long`.
const fn primitive_int(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => "byte",
        ir::IntKind::I16 | ir::IntKind::U16 => "short",
        ir::IntKind::I32 | ir::IntKind::U32 => "int",
        ir::IntKind::I64
        | ir::IntKind::U64
        | ir::IntKind::Isize
        | ir::IntKind::Usize => "long",
    }
}

const fn boxed_int(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => "Byte",
        ir::IntKind::I16 | ir::IntKind::U16 => "Short",
        ir::IntKind::I32 | ir::IntKind::U32 => "Integer",
        ir::IntKind::I64
        | ir::IntKind::U64
        | ir::IntKind::Isize
        | ir::IntKind::Usize => "Long",
    }
}

/// The reader method decoding one integer kind.
const fn read_int_method(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => "readByte",
        ir::IntKind::I16 | ir::IntKind::U16 => "readShort",
        ir::IntKind::I32 | ir::IntKind::U32 => "readInt",
        ir::IntKind::I64
        | ir::IntKind::U64
        | ir::IntKind::Isize
        | ir::IntKind::Usize => "readLong",
    }
}

/// The writer method encoding one integer kind.
const fn write_int_method(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => "writeByte",
        ir::IntKind::I16 | ir::IntKind::U16 => "writeShort",
        ir::IntKind::I32 | ir::IntKind::U32 => "writeInt",
        ir::IntKind::I64
        | ir::IntKind::U64
        | ir::IntKind::Isize
        | ir::IntKind::Usize => "writeLong",
    }
}

/// A Java expression decoding one value of `ty` from the reader named
/// `reader`. `depth` uniquifies the lambda parameters of nested containers.
pub fn decode(ty: &ir::Type, interface: &ir::Interface, reader: &str, depth: usize) -> String {
    match ty {
        ir::Type::Bool => format!("{reader}.readBool()"),
        ir::Type::Int(kind) => format!("{reader}.{}()", read_int_method(*kind)),
        ir::Type::Float(ir::FloatKind::F32) => format!("{reader}.readFloat()"),
        ir::Type::Float(ir::FloatKind::F64) => format!("{reader}.readDouble()"),
        ir::Type::String { .. } => format!("{reader}.readString()"),
        ir::Type::Path { .. } => format!("Path.of({reader}.readString())"),
        ir::Type::Bytes { .. } => format!("{reader}.readBytes()"),
        ir::Type::Option(inner) => {
            let inner = decode(inner, interface, reader, depth);
            format!("({reader}.readBool() ? {inner} : null)")
        }
        ir::Type::Vec(inner) => {
            let param = format!("w{depth}");
            let inner = decode(inner, interface, &param, depth + 1);
            format!("readList({reader}, {param} -> {inner})")
        }
        ir::Type::Map { key, value } => {
            let param = format!("w{depth}");
            let key = decode(key, interface, &param, depth + 1);
            let value = decode(value, interface, &param, depth + 1);
            format!("readMap({reader}, {param} -> {key}, {param} -> {value})")
        }
        ir::Type::Named(name) => {
            let record = names::record_name_of(interface, name);
            format!("read{record}({reader})")
        }
        ir::Type::Stream(_) => unreachable!("rejected by check_boundary"),
    }
}

/// Java statements encoding the value spelled `place` (of type `ty`) into
/// the writer named `wire`. Lines carry four spaces of indent per nesting
/// level relative to the snippet; [`indent`] shifts the whole snippet to
/// its insertion point.
pub fn encode(
    ty: &ir::Type,
    interface: &ir::Interface,
    place: &str,
    wire: &str,
    depth: usize,
) -> String {
    match ty {
        ir::Type::Bool => format!("{wire}.writeBool({place});"),
        ir::Type::Int(kind) => format!("{wire}.{}({place});", write_int_method(*kind)),
        ir::Type::Float(ir::FloatKind::F32) => format!("{wire}.writeFloat({place});"),
        ir::Type::Float(ir::FloatKind::F64) => format!("{wire}.writeDouble({place});"),
        ir::Type::String { .. } => format!("{wire}.writeString({place});"),
        ir::Type::Path { .. } => format!("{wire}.writeString({place}.toString());"),
        ir::Type::Bytes { .. } => format!("{wire}.writeBytes({place});"),
        ir::Type::Option(inner) => {
            let inner = indent(&encode(inner, interface, place, wire, depth), 1);
            format!(
                "if ({place} == null) {{\n    {wire}.writeBool(false);\n}} else {{\n    \
                 {wire}.writeBool(true);\n{inner}\n}}"
            )
        }
        ir::Type::Vec(inner) => {
            let writer = format!("w{depth}");
            let element = format!("v{depth}");
            let inner = indent(&encode(inner, interface, &element, &writer, depth + 1), 1);
            format!("writeList({wire}, {place}, ({writer}, {element}) -> {{\n{inner}\n}});")
        }
        ir::Type::Map { key, value } => {
            let writer = format!("w{depth}");
            let key_binding = format!("k{depth}");
            let value_binding = format!("v{depth}");
            let key = indent(&encode(key, interface, &key_binding, &writer, depth + 1), 1);
            let value = indent(
                &encode(value, interface, &value_binding, &writer, depth + 1),
                1,
            );
            format!(
                "writeMap({wire}, {place}, ({writer}, {key_binding}) -> {{\n{key}\n}}, \
                 ({writer}, {value_binding}) -> {{\n{value}\n}});"
            )
        }
        ir::Type::Named(name) => {
            let record = names::record_name_of(interface, name);
            format!("write{record}({wire}, {place});")
        }
        ir::Type::Stream(_) => unreachable!("rejected by check_boundary"),
    }
}

/// Shift every line of `snippet` right by `levels` four-space steps.
pub fn indent(snippet: &str, levels: usize) -> String {
    let pad = "    ".repeat(levels);
    snippet
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A Java string literal spelling `value`.
pub fn string_literal(value: &str) -> String {
    let mut out = String::from('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A javadoc block from pre-escaped lines; empty input renders nothing.
/// The block ends with a newline, ready to sit above a declaration.
pub fn javadoc(lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut out = String::from("/**\n");
    for line in lines {
        if line.is_empty() {
            out.push_str(" *\n");
        } else {
            out.push_str(" * ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(" */\n");
    out
}

/// A javadoc `{@code ...}` span.
pub fn code(text: &str) -> String {
    format!("{{@code {text}}}")
}

/// Escape doc-comment text for a javadoc block.
pub fn javadoc_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace("*/", "*&#47;")
}

/// `java_ty` without its generic arguments, as `{@link}` references need.
pub fn strip_generics(java_ty: &str) -> &str {
    java_ty.split_once('<').map_or(java_ty, |(raw, _)| raw)
}
