//! The private encode helpers of the module class: one per aggregate
//! mirror reachable from argument values.
//!
//! Helpers write into zero-initialized arena memory (`Arena#allocate`
//! zeroes), which is what makes "absent option means all-zero value" hold
//! without explicit clearing.

use std::collections::BTreeMap;

use crate::ctype::CTy;
use crate::java::{line, types};
use crate::model::Model;

/// Render every needed encode helper, ordered by mangled name.
pub fn helpers(model: &Model<'_>, set: &BTreeMap<String, CTy>) -> String {
    let mut out = String::new();
    for ty in set.values() {
        let text = helper(model, ty);
        if !text.is_empty() {
            out.push('\n');
            out.push_str(&text);
        }
    }
    out
}

/// The Java statement (no trailing `;`) writing one mirror value at
/// `offset` inside `seg`.
pub fn write_stmt(ty: &CTy, seg: &str, offset: &str, value: &str) -> String {
    if ty.is_scalar() {
        types::scalar_set(ty, seg, offset, value)
    } else {
        format!("encode{}(arena, {seg}, {offset}, {value})", ty.mangle())
    }
}

fn helper(model: &Model<'_>, ty: &CTy) -> String {
    match ty {
        CTy::Str => str_helper(),
        CTy::Path => path_helper(),
        CTy::Bytes => bytes_helper(),
        CTy::Option(inner) => option_helper(model, ty, inner),
        CTy::Vec(inner) => list_helper(model, ty, inner),
        CTy::Map { key, value } => map_helper(model, ty, key, value),
        CTy::Record(name) => record_helper(model, name),
        CTy::Bool | CTy::Int(_) | CTy::Float(_) => String::new(),
    }
}

fn str_helper() -> String {
    let mut out = String::new();
    line(
        &mut out,
        1,
        "private static void encodeStr(Arena arena, MemorySegment seg, long offset, String value) {",
    );
    line(&mut out, 2, "byte[] bytes = value.getBytes(StandardCharsets.UTF_8);");
    line(&mut out, 2, "MemorySegment data = arena.allocateFrom(ValueLayout.JAVA_BYTE, bytes);");
    line(&mut out, 2, "seg.set(ValueLayout.ADDRESS, offset, data);");
    line(&mut out, 2, "seg.set(ValueLayout.JAVA_LONG, offset + 8, bytes.length);");
    line(&mut out, 1, "}");
    out
}

fn path_helper() -> String {
    let mut out = String::new();
    line(
        &mut out,
        1,
        "private static void encodePath(Arena arena, MemorySegment seg, long offset, java.nio.file.Path value) {",
    );
    line(&mut out, 2, "encodeStr(arena, seg, offset, value.toString());");
    line(&mut out, 1, "}");
    out
}

fn bytes_helper() -> String {
    let mut out = String::new();
    line(
        &mut out,
        1,
        "private static void encodeBytes(Arena arena, MemorySegment seg, long offset, byte[] value) {",
    );
    line(&mut out, 2, "MemorySegment data = arena.allocateFrom(ValueLayout.JAVA_BYTE, value);");
    line(&mut out, 2, "seg.set(ValueLayout.ADDRESS, offset, data);");
    line(&mut out, 2, "seg.set(ValueLayout.JAVA_LONG, offset + 8, value.length);");
    line(&mut out, 1, "}");
    out
}

fn option_helper(model: &Model<'_>, ty: &CTy, inner: &CTy) -> String {
    let java = types::java_type(inner, true);
    let value_offset = types::offset_expr("offset", model.option_value_offset(inner));
    let write = write_stmt(inner, "seg", &value_offset, "value");
    let mut out = String::new();
    line(
        &mut out,
        1,
        &format!(
            "private static void encode{}(Arena arena, MemorySegment seg, long offset, {java} value) {{",
            ty.mangle()
        ),
    );
    line(&mut out, 2, "if (value == null) {");
    line(&mut out, 3, "return;");
    line(&mut out, 2, "}");
    line(&mut out, 2, "seg.set(ValueLayout.JAVA_BYTE, offset, (byte) 1);");
    line(&mut out, 2, &format!("{write};"));
    line(&mut out, 1, "}");
    out
}

fn list_helper(model: &Model<'_>, ty: &CTy, inner: &CTy) -> String {
    let boxed = types::java_type(inner, true);
    let layout = model.layout(inner);
    let stride = layout.size;
    let write = write_stmt(inner, "data", "cursor", "element");
    let mut out = String::new();
    line(
        &mut out,
        1,
        &format!(
            "private static void encode{}(Arena arena, MemorySegment seg, long offset, java.util.List<{boxed}> value) {{",
            ty.mangle()
        ),
    );
    line(
        &mut out,
        2,
        &format!("MemorySegment data = arena.allocate({stride} * (long) value.size(), {});", layout.align),
    );
    line(&mut out, 2, "long cursor = 0;");
    line(&mut out, 2, &format!("for ({boxed} element : value) {{"));
    line(&mut out, 3, &format!("{write};"));
    line(&mut out, 3, &format!("cursor += {stride};"));
    line(&mut out, 2, "}");
    line(&mut out, 2, "seg.set(ValueLayout.ADDRESS, offset, data);");
    line(&mut out, 2, "seg.set(ValueLayout.JAVA_LONG, offset + 8, value.size());");
    line(&mut out, 1, "}");
    out
}

fn map_helper(model: &Model<'_>, ty: &CTy, key: &CTy, value: &CTy) -> String {
    let key_java = types::java_type(key, true);
    let value_java = types::java_type(value, true);
    let pair = model.pair_struct(key, value);
    let stride = pair.layout.size;
    let key_write = write_stmt(key, "data", &types::offset_expr("cursor", pair.offsets[0]), "entry.getKey()");
    let value_write = write_stmt(
        value,
        "data",
        &types::offset_expr("cursor", pair.offsets[1]),
        "entry.getValue()",
    );
    let mut out = String::new();
    line(
        &mut out,
        1,
        &format!(
            "private static void encode{}(Arena arena, MemorySegment seg, long offset, java.util.Map<{key_java}, {value_java}> value) {{",
            ty.mangle()
        ),
    );
    line(
        &mut out,
        2,
        &format!("MemorySegment data = arena.allocate({stride} * (long) value.size(), {});", pair.layout.align),
    );
    line(&mut out, 2, "long cursor = 0;");
    line(
        &mut out,
        2,
        &format!("for (java.util.Map.Entry<{key_java}, {value_java}> entry : value.entrySet()) {{"),
    );
    line(&mut out, 3, &format!("{key_write};"));
    line(&mut out, 3, &format!("{value_write};"));
    line(&mut out, 3, &format!("cursor += {stride};"));
    line(&mut out, 2, "}");
    line(&mut out, 2, "seg.set(ValueLayout.ADDRESS, offset, data);");
    line(&mut out, 2, "seg.set(ValueLayout.JAVA_LONG, offset + 8, value.size());");
    line(&mut out, 1, "}");
    out
}

fn record_helper(model: &Model<'_>, name: &str) -> String {
    let record = model.record(name);
    let shape = model.record_struct(name);
    let mut out = String::new();
    line(
        &mut out,
        1,
        &format!("private static void encode{name}(Arena arena, MemorySegment seg, long offset, {name} value) {{"),
    );
    for (field, offset) in record.fields.iter().zip(&shape.offsets) {
        let cty = CTy::of(&field.ty);
        let accessor = format!("value.{}()", crate::names::camel(&field.name));
        let write = write_stmt(&cty, "seg", &types::offset_expr("offset", *offset), &accessor);
        line(&mut out, 2, &format!("{write};"));
    }
    line(&mut out, 1, "}");
    out
}
