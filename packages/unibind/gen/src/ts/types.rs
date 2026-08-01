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

/// The integer widths that cross as a JavaScript `BigInt`: a `number` is an
/// IEEE double, exact only to 2^53, so the widths past that cross as
/// `bigint` in every position, including record fields and container
/// elements, matching the glue. Every TypeScript renderer asks here, so the
/// declared type in `index.d.ts` and the Zod schema in `schemas.ts` cannot
/// disagree about a width.
pub const fn crosses_as_bigint(kind: ir::IntKind) -> bool {
    matches!(
        kind,
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize
    )
}

/// The shared refusal for a map the ts backend cannot key.
pub fn integer_keyed_map() -> EmitError {
    EmitError {
        message: "integer-keyed maps are not part of the ts backend yet (issue #1993)".to_owned(),
    }
}

/// The TypeScript type of a value crossing at `level`.
///
/// # Errors
///
/// Fails for the surface the compiled glue also rejects (integer-keyed
/// maps, nested streams), so it only trips on IR that never compiled
/// through the ts macro backend.
pub fn ts_type(
    interface: &ir::Interface,
    ty: &ir::Type,
    level: Level,
) -> Result<String, EmitError> {
    Ok(match ty {
        ir::Type::Bool => "boolean".to_owned(),
        ir::Type::Int(kind) => {
            if crosses_as_bigint(*kind) {
                "bigint".to_owned()
            } else {
                "number".to_owned()
            }
        }
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
                return Err(integer_keyed_map());
            }
            format!(
                "Record<string, {}>",
                ts_type(interface, value, Level::Nested)?
            )
        }
        ir::Type::Named(name) => named_type_name(interface, name).to_owned(),
        ir::Type::Stream(_) => {
            return Err(EmitError {
                message: "streams cross only as a whole function return type".to_owned(),
            });
        }
    })
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

/// Every callable the host files render a signature for: the free
/// functions and each object's methods.
fn callables(interface: &ir::Interface) -> impl Iterator<Item = &ir::Function> {
    let methods = interface
        .objects
        .iter()
        .flat_map(|object| object.methods.iter());
    interface.functions.iter().chain(methods)
}

/// Whether any signature spells `Buffer`, which pulls the `node:buffer`
/// type import into `index.d.ts`.
pub fn uses_buffer(interface: &ir::Interface) -> bool {
    callables(interface).any(|function| {
        function.args.iter().any(|arg| top_level_bytes(&arg.ty))
            || function.ret.as_ref().is_some_and(top_level_bytes)
    })
}

/// Whether any export returns a stream, which pulls the `UnibindStream`
/// declaration into `index.d.ts` and the `wrapStream` helper into
/// `index.js`. Methods count here: the ts backend renders stream methods,
/// where `ir::Interface::has_streams` answers the free-function-only
/// question the backends that reject them ask.
pub fn has_streams(interface: &ir::Interface) -> bool {
    callables(interface).any(|function| matches!(function.ret, Some(ir::Type::Stream(_))))
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
