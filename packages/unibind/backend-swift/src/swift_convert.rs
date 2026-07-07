//! Map boundary types to Swift spellings and generate the conversion code
//! between native Swift values and the bridge's low-level types.

use unibind_core::ir;

use crate::names;
use crate::repr::{self, BoxShape, Repr, Scalar};

/// The native Swift type for an IR type.
pub fn swift_type(ty: &ir::Type) -> String {
    match ty {
        ir::Type::Bool => "Bool".to_owned(),
        ir::Type::Int(kind) => int_name(*kind).to_owned(),
        ir::Type::Float(ir::FloatKind::F32) => "Float".to_owned(),
        ir::Type::Float(ir::FloatKind::F64) => "Double".to_owned(),
        ir::Type::String { .. } | ir::Type::Path { .. } => "String".to_owned(),
        ir::Type::Bytes { .. } => "[UInt8]".to_owned(),
        ir::Type::Option(inner) => format!("{}?", swift_type(inner)),
        ir::Type::Vec(inner) => format!("[{}]", swift_type(inner)),
        ir::Type::Map { key, value } => {
            format!("[{}: {}]", swift_type(key), swift_type(value))
        }
        ir::Type::Named(name) => name.clone(),
        ir::Type::Stream(_) => unreachable!("streams are rejected before type mapping"),
    }
}

fn int_name(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 => "Int8",
        ir::IntKind::I16 => "Int16",
        ir::IntKind::I32 => "Int32",
        ir::IntKind::I64 => "Int64",
        ir::IntKind::Isize => "Int",
        ir::IntKind::U8 => "UInt8",
        ir::IntKind::U16 => "UInt16",
        ir::IntKind::U32 => "UInt32",
        ir::IntKind::U64 => "UInt64",
        ir::IntKind::Usize => "UInt",
    }
}

fn scalar_name(scalar: Scalar) -> &'static str {
    match scalar {
        Scalar::Bool => "Bool",
        Scalar::Int(kind) => int_name(kind),
        Scalar::Float(ir::FloatKind::F32) => "Float",
        Scalar::Float(ir::FloatKind::F64) => "Double",
    }
}

/// A Swift literal for a default value.
pub fn literal(value: &ir::Literal) -> String {
    match value {
        ir::Literal::Bool(true) => "true".to_owned(),
        ir::Literal::Bool(false) => "false".to_owned(),
        ir::Literal::Int(int) => int.to_string(),
        // `{:?}` keeps a fractional part (`1.0`, not `1`), so the rendered
        // default stays a Swift Double literal.
        ir::Literal::Float(float) => format!("{float:?}"),
        ir::Literal::Str(text) => str_literal(text),
        ir::Literal::None => "nil".to_owned(),
    }
}

/// A double-quoted Swift string literal.
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

/// Conversion of one native Swift value into what the bridge call takes:
/// statements to emit before the call plus the argument expression.
pub struct ToBridge {
    pub lines: Vec<String>,
    pub expr: String,
}

/// Convert `expr` (a native Swift value of `ty`) for the bridge. `temp`
/// seeds unique local names; `indent` is the emission depth of `lines`.
pub fn to_bridge(ty: &ir::Type, expr: &str, temp: &str, indent: usize) -> ToBridge {
    let pad = "    ".repeat(indent);
    match repr::repr_of(ty) {
        Repr::Scalar(_) | Repr::OptionScalar(_) | Repr::OptionStr | Repr::Str => ToBridge {
            lines: Vec::new(),
            expr: expr.to_owned(),
        },
        Repr::Bytes | Repr::VecScalar(_) => {
            let element = match repr::repr_of(ty) {
                Repr::Bytes => "UInt8".to_owned(),
                Repr::VecScalar(scalar) => scalar_name(scalar).to_owned(),
                _ => unreachable!("matched above"),
            };
            let lines = vec![
                format!("{pad}let {temp} = RustVec<{element}>()"),
                format!("{pad}for __element in {expr} {{"),
                format!("{pad}    {temp}.push(value: __element)"),
                format!("{pad}}}"),
            ];
            ToBridge {
                lines,
                expr: temp.to_owned(),
            }
        }
        Repr::Boxed(BoxShape::Record(_)) => ToBridge {
            lines: Vec::new(),
            expr: format!("{expr}.__unibindHandle()"),
        },
        Repr::Boxed(shape) => boxed_to_bridge(&shape, expr, temp, indent, &pad),
    }
}

fn boxed_to_bridge(
    shape: &BoxShape,
    expr: &str,
    temp: &str,
    indent: usize,
    pad: &str,
) -> ToBridge {
    let handle = shape.ident().to_string();
    let ctor = names::to_snake(&shape.mangle());
    match shape {
        BoxShape::Vec(inner) => {
            let element = to_bridge(inner, "__element", &format!("{temp}_e"), indent + 1);
            let mut lines = vec![
                format!("{pad}let {temp} = __unibind_new_{ctor}()"),
                format!("{pad}for __element in {expr} {{"),
            ];
            lines.extend(element.lines);
            lines.push(format!("{pad}    {temp}.push({})", element.expr));
            lines.push(format!("{pad}}}"));
            ToBridge {
                lines,
                expr: temp.to_owned(),
            }
        }
        BoxShape::Option(inner) => {
            let payload = to_bridge(inner, "__payload", &format!("{temp}_p"), indent + 1);
            let mut lines = vec![
                format!("{pad}let {temp}: {handle}"),
                format!("{pad}if let __payload = {expr} {{"),
            ];
            lines.extend(payload.lines);
            lines.push(format!(
                "{pad}    {temp} = __unibind_new_{ctor}_some({})",
                payload.expr
            ));
            lines.push(format!("{pad}}} else {{"));
            lines.push(format!("{pad}    {temp} = __unibind_new_{ctor}_none()"));
            lines.push(format!("{pad}}}"));
            ToBridge {
                lines,
                expr: temp.to_owned(),
            }
        }
        BoxShape::Map { key, value } => {
            let key_conv = to_bridge(key, "__key", &format!("{temp}_k"), indent + 1);
            let value_conv = to_bridge(value, "__value", &format!("{temp}_v"), indent + 1);
            let mut lines = vec![
                format!("{pad}let {temp} = __unibind_new_{ctor}()"),
                format!("{pad}for (__key, __value) in {expr} {{"),
            ];
            lines.extend(key_conv.lines);
            lines.extend(value_conv.lines);
            lines.push(format!(
                "{pad}    {temp}.insert({}, {})",
                key_conv.expr, value_conv.expr
            ));
            lines.push(format!("{pad}}}"));
            ToBridge {
                lines,
                expr: temp.to_owned(),
            }
        }
        BoxShape::Record(_) => unreachable!("records convert via __unibindHandle"),
        // Value carriers only appear in throwing return position; arguments
        // never route through them.
        BoxShape::Value(_) => unreachable!("value carriers never appear in argument position"),
    }
}

/// A Swift expression rebuilding the native value of `ty` from the
/// low-level bridge value `expr` (closures cover the loop cases).
pub fn from_bridge(ty: &ir::Type, expr: &str) -> String {
    let native = swift_type(ty);
    match repr::repr_of(ty) {
        Repr::Scalar(_) | Repr::OptionScalar(_) => expr.to_owned(),
        Repr::Str => format!("{expr}.toString()"),
        Repr::OptionStr => format!("{expr}.map {{ $0.toString() }}"),
        // RustVec.len() is `Int` while get(index:) takes `UInt`, so the
        // index converts at the range.
        Repr::Bytes | Repr::VecScalar(_) => format!(
            "{{ () -> {native} in let __vec = {expr}; var __out: {native} = []; \
for __index in 0..<UInt(__vec.len()) {{ __out.append(__vec.get(index: __index)!) }}; \
return __out }}()"
        ),
        Repr::Boxed(BoxShape::Record(name)) => format!("{name}(__handle: {expr})"),
        Repr::Boxed(BoxShape::Vec(inner)) => {
            let element = from_bridge(&inner, "__box.get(__index)");
            format!(
                "{{ () -> {native} in let __box = {expr}; var __out: {native} = []; \
for __index in 0..<__box.len() {{ __out.append({element}) }}; return __out }}()"
            )
        }
        Repr::Boxed(BoxShape::Option(inner)) => {
            let payload = from_bridge(&inner, "__box.value()");
            format!(
                "{{ () -> {native} in let __box = {expr}; \
if __box.is_some() {{ return {payload} }}; return nil }}()"
            )
        }
        Repr::Boxed(BoxShape::Map { key, value }) => {
            let key_expr = from_bridge(&key, "__box.key_at(__index)");
            let value_expr = from_bridge(&value, "__box.value_at(__index)");
            format!(
                "{{ () -> {native} in let __box = {expr}; var __out: {native} = [:]; \
for __index in 0..<__box.len() {{ __out[{key_expr}] = {value_expr} }}; return __out }}()"
            )
        }
        Repr::Boxed(BoxShape::Value(_)) => {
            unreachable!("value carriers are drained before conversion")
        }
    }
}
