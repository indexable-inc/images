//! Naming and TypeScript type rendering, mirroring the Rust-side mapping
//! the `unibind-backend-ts` glue compiled into the addon.

use unibind_core::ir;

use crate::host::EmitError;

/// How close to a signature a type sits. `Buffer` only replaces bytes at
/// the top level of arguments and returns (including directly under
/// `Option` and as a stream element); nested bytes cross as plain number
/// arrays, matching the glue's `Vec<u8>` fields and elements.
#[derive(Clone, Copy)]
pub enum Level {
    Top,
    Nested,
}

/// napi's automatic `snake_case` -> `camelCase` conversion, applied to every
/// unrenamed function, method, argument, and record field name.
pub fn camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper_next = false;
    for character in name.chars() {
        if character == '_' {
            upper_next = !out.is_empty();
        } else if upper_next {
            out.extend(character.to_uppercase());
            upper_next = false;
        } else {
            out.push(character);
        }
    }
    out
}

/// The JavaScript name of a value item (function, method, argument, or
/// field): the ts rename verbatim, else the camelCased Rust name.
pub fn value_name(name: &str, names: &ir::Names) -> String {
    names.ts.clone().unwrap_or_else(|| camel_case(name))
}

/// The JavaScript name of a type (record, error enum or variant, object):
/// the ts rename verbatim, else the Rust name, which is already
/// `PascalCase`.
pub fn type_name<'a>(names: &'a ir::Names, name: &'a str) -> &'a str {
    names.ts.as_deref().unwrap_or(name)
}

/// The TypeScript type of a value crossing at `level`.
///
/// # Errors
///
/// Fails for the surface the compiled glue also rejects (BigInt-only
/// integers, integer-keyed maps, nested streams), so it only trips on IR
/// that never compiled through the ts macro backend.
pub fn ts_type(
    interface: &ir::Interface,
    ty: &ir::Type,
    level: Level,
) -> Result<String, EmitError> {
    Ok(match ty {
        ir::Type::Bool => "boolean".to_owned(),
        ir::Type::Int(kind) => match kind {
            ir::IntKind::U64 | ir::IntKind::Usize | ir::IntKind::Isize => {
                return Err(EmitError {
                    message: format!(
                        "`{}` only crosses as a BigInt; the ts backend rejects it \
                         until BigInt lands (issue #1993)",
                        int_name(*kind)
                    ),
                });
            }
            _ => "number".to_owned(),
        },
        ir::Type::Float(_) => "number".to_owned(),
        ir::Type::String { .. } | ir::Type::Path { .. } => "string".to_owned(),
        ir::Type::Bytes { .. } => match level {
            Level::Top => "Buffer".to_owned(),
            Level::Nested => "Array<number>".to_owned(),
        },
        ir::Type::Option(inner) => format!("{} | null", ts_type(interface, inner, level)?),
        ir::Type::Vec(inner) => {
            format!("Array<{}>", ts_type(interface, inner, Level::Nested)?)
        }
        ir::Type::Map { key, value } => {
            if !matches!(**key, ir::Type::String { .. }) {
                return Err(EmitError {
                    message: "integer-keyed maps are not part of the ts backend yet \
                              (issue #1993)"
                        .to_owned(),
                });
            }
            format!("Record<string, {}>", ts_type(interface, value, Level::Nested)?)
        }
        ir::Type::Named(name) => named_type_name(interface, name).to_owned(),
        ir::Type::Stream(_) => {
            return Err(EmitError {
                message: "streams cross only as a whole function return type".to_owned(),
            });
        }
    })
}

const fn int_name(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 => "i8",
        ir::IntKind::I16 => "i16",
        ir::IntKind::I32 => "i32",
        ir::IntKind::I64 => "i64",
        ir::IntKind::Isize => "isize",
        ir::IntKind::U8 => "u8",
        ir::IntKind::U16 => "u16",
        ir::IntKind::U32 => "u32",
        ir::IntKind::U64 => "u64",
        ir::IntKind::Usize => "usize",
    }
}

/// Resolve a `Named` reference (a record or an object) to its JavaScript
/// name.
fn named_type_name<'a>(interface: &'a ir::Interface, name: &'a str) -> &'a str {
    if let Some(record) = interface.records.iter().find(|record| record.name == name) {
        return type_name(&record.names, &record.name);
    }
    interface
        .objects
        .iter()
        .find(|object| object.name == name)
        .map_or(name, |object| type_name(&object.names, &object.name))
}

/// Whether the interface returns any stream, which pulls the shared
/// `UnibindStream<T>` shape into both emitted files.
pub fn has_streams(interface: &ir::Interface) -> bool {
    interface
        .functions
        .iter()
        .any(|function| matches!(function.ret, Some(ir::Type::Stream(_))))
}

/// Whether any signature spells `Buffer`, which pulls the `node:buffer`
/// type import into `index.d.ts`.
pub fn uses_buffer(interface: &ir::Interface) -> bool {
    let methods = interface
        .objects
        .iter()
        .flat_map(|object| object.methods.iter());
    interface.functions.iter().chain(methods).any(|function| {
        function.args.iter().any(|arg| top_level_bytes(&arg.ty))
            || function.ret.as_ref().is_some_and(top_level_bytes)
    })
}

fn top_level_bytes(ty: &ir::Type) -> bool {
    match ty {
        ir::Type::Bytes { .. } => true,
        ir::Type::Option(inner) | ir::Type::Stream(inner) => top_level_bytes(inner),
        _ => false,
    }
}

/// Append a TSDoc/JSDoc block for `lines` at `indent`.
pub fn doc_block(out: &mut String, indent: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    if let [line] = lines {
        out.push_str(indent);
        out.push_str("/** ");
        out.push_str(line.trim());
        out.push_str(" */\n");
        return;
    }
    out.push_str(indent);
    out.push_str("/**\n");
    for line in lines {
        let line = line.trim_end();
        out.push_str(indent);
        if line.is_empty() {
            out.push_str(" *\n");
        } else {
            out.push_str(" * ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(indent);
    out.push_str(" */\n");
}

/// The user close method the resource surface wraps: named `close`, zero
/// arguments, no success value (the shape lowering guarantees resources
/// declare). `None` for plain objects.
pub fn resource_close(object: &ir::Object) -> Option<&ir::Function> {
    if !object.resource {
        return None;
    }
    object
        .methods
        .iter()
        .find(|method| method.name == "close" && method.args.is_empty() && method.ret.is_none())
}
