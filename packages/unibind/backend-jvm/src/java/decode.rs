//! The private decode helpers of the module class: one per aggregate
//! mirror reachable from return values.
//!
//! Every offset baked here comes from [`crate::model::Model`], the same
//! numbers the Rust glue asserts at compile time.

use std::collections::BTreeMap;

use crate::ctype::CTy;
use crate::java::{line, types};
use crate::model::Model;

/// Render every needed decode helper, ordered by mangled name.
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

/// The Java expression reading one mirror at `offset` inside `seg`.
pub fn read_expr(ty: &CTy, seg: &str, offset: &str) -> String {
    if ty.is_scalar() {
        types::scalar_get(ty, seg, offset)
    } else {
        format!("decode{}({seg}, {offset})", ty.mangle())
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
    line(&mut out, 1, "private static String decodeStr(MemorySegment seg, long offset) {");
    line(&mut out, 2, "long len = seg.get(ValueLayout.JAVA_LONG, offset + 8);");
    line(&mut out, 2, "if (len == 0) {");
    line(&mut out, 3, "return \"\";");
    line(&mut out, 2, "}");
    line(
        &mut out,
        2,
        "byte[] bytes = seg.get(ValueLayout.ADDRESS, offset).reinterpret(len).toArray(ValueLayout.JAVA_BYTE);",
    );
    line(&mut out, 2, "return new String(bytes, StandardCharsets.UTF_8);");
    line(&mut out, 1, "}");
    out
}

fn bytes_helper() -> String {
    let mut out = String::new();
    line(&mut out, 1, "private static byte[] decodeBytes(MemorySegment seg, long offset) {");
    line(&mut out, 2, "long len = seg.get(ValueLayout.JAVA_LONG, offset + 8);");
    line(&mut out, 2, "if (len == 0) {");
    line(&mut out, 3, "return new byte[0];");
    line(&mut out, 2, "}");
    line(
        &mut out,
        2,
        "return seg.get(ValueLayout.ADDRESS, offset).reinterpret(len).toArray(ValueLayout.JAVA_BYTE);",
    );
    line(&mut out, 1, "}");
    out
}

fn path_helper() -> String {
    let mut out = String::new();
    line(
        &mut out,
        1,
        "private static java.nio.file.Path decodePath(MemorySegment seg, long offset) {",
    );
    line(&mut out, 2, "return java.nio.file.Path.of(decodeStr(seg, offset));");
    line(&mut out, 1, "}");
    out
}

fn option_helper(model: &Model<'_>, ty: &CTy, inner: &CTy) -> String {
    let java = types::java_type(inner, true);
    let value_offset = types::offset_expr("offset", model.option_value_offset(inner));
    let read = read_expr(inner, "seg", &value_offset);
    let mut out = String::new();
    line(
        &mut out,
        1,
        &format!("private static {java} decode{}(MemorySegment seg, long offset) {{", ty.mangle()),
    );
    line(&mut out, 2, "if (seg.get(ValueLayout.JAVA_BYTE, offset) == 0) {");
    line(&mut out, 3, "return null;");
    line(&mut out, 2, "}");
    line(&mut out, 2, &format!("return {read};"));
    line(&mut out, 1, "}");
    out
}

fn list_helper(model: &Model<'_>, ty: &CTy, inner: &CTy) -> String {
    let boxed = types::java_type(inner, true);
    let stride = model.layout(inner).size;
    let read = read_expr(inner, "data", &format!("index * {stride}"));
    let mut out = String::new();
    line(
        &mut out,
        1,
        &format!(
            "private static java.util.List<{boxed}> decode{}(MemorySegment seg, long offset) {{",
            ty.mangle()
        ),
    );
    line(&mut out, 2, "long len = seg.get(ValueLayout.JAVA_LONG, offset + 8);");
    line(&mut out, 2, &format!("java.util.List<{boxed}> list = new java.util.ArrayList<>();"));
    line(&mut out, 2, "if (len == 0) {");
    line(&mut out, 3, "return list;");
    line(&mut out, 2, "}");
    line(
        &mut out,
        2,
        &format!("MemorySegment data = seg.get(ValueLayout.ADDRESS, offset).reinterpret(len * {stride});"),
    );
    line(&mut out, 2, "for (long index = 0; index < len; index++) {");
    line(&mut out, 3, &format!("list.add({read});"));
    line(&mut out, 2, "}");
    line(&mut out, 2, "return list;");
    line(&mut out, 1, "}");
    out
}

fn map_helper(model: &Model<'_>, ty: &CTy, key: &CTy, value: &CTy) -> String {
    let key_java = types::java_type(key, true);
    let value_java = types::java_type(value, true);
    let pair = model.pair_struct(key, value);
    let stride = pair.layout.size;
    let key_read = read_expr(key, "data", &types::offset_expr(&format!("index * {stride}"), pair.offsets[0]));
    let value_read = read_expr(value, "data", &types::offset_expr(&format!("index * {stride}"), pair.offsets[1]));
    let mut out = String::new();
    line(
        &mut out,
        1,
        &format!(
            "private static java.util.Map<{key_java}, {value_java}> decode{}(MemorySegment seg, long offset) {{",
            ty.mangle()
        ),
    );
    line(&mut out, 2, "long len = seg.get(ValueLayout.JAVA_LONG, offset + 8);");
    line(
        &mut out,
        2,
        &format!("java.util.Map<{key_java}, {value_java}> map = new java.util.LinkedHashMap<>();"),
    );
    line(&mut out, 2, "if (len == 0) {");
    line(&mut out, 3, "return map;");
    line(&mut out, 2, "}");
    line(
        &mut out,
        2,
        &format!("MemorySegment data = seg.get(ValueLayout.ADDRESS, offset).reinterpret(len * {stride});"),
    );
    line(&mut out, 2, "for (long index = 0; index < len; index++) {");
    line(&mut out, 3, &format!("map.put({key_read}, {value_read});"));
    line(&mut out, 2, "}");
    line(&mut out, 2, "return map;");
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
        &format!("private static {name} decode{name}(MemorySegment seg, long offset) {{"),
    );
    line(&mut out, 2, &format!("return new {name}("));
    let reads: Vec<String> = record
        .fields
        .iter()
        .zip(&shape.offsets)
        .map(|(field, offset)| {
            let cty = CTy::of(&field.ty);
            format!(
                "                {}",
                read_expr(&cty, "seg", &types::offset_expr("offset", *offset))
            )
        })
        .collect();
    out.push_str(&reads.join(",\n"));
    out.push_str(");\n");
    line(&mut out, 1, "}");
    out
}
