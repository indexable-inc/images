//! Naming and TypeScript type rendering, mirroring the Rust-side mapping the
//! glue compiled into the artifact.
//!
//! Both JavaScript flavors declare one vocabulary, so the mapping lives here
//! once and asks [`Flavor`] for the positions where napi and `wasm-bindgen`
//! genuinely carry a value differently.

use unibind_core::ir;

use super::flavor::{self, Flavor};
use crate::host::EmitError;

/// Which position a value occupies, which is what decides how bytes cross.
/// The twin of `unibind_backend_ts::ty::Level`, and it has to answer
/// identically: the glue compiled into the artifact is what actually
/// crosses, and these declarations only describe it.
///
/// The dividing line is not depth. A whole argument or return value is what
/// the binding library carries itself; a record field is what the mirror
/// struct (napi) or the serde twin (`wasm-bindgen`) declares; a `Vec`
/// element or a map value is interior to a container that crosses whole.
/// Only bytes tell the three apart, and only the middle one differs between
/// the flavors.
#[derive(Clone, Copy)]
pub enum Level {
    /// A whole argument, return value, or stream item.
    Top,
    /// A field of a record.
    Field,
    /// Inside a container: a `Vec` element or a map value.
    Element,
}

/// The byte-string spelling at `level`. Both renderers (`index.d.ts` and
/// `schemas.ts`) ask here rather than matching the variants, so a declared
/// type and its schema cannot disagree about one position.
const fn bytes_type(flavor: &Flavor, level: Level) -> &'static str {
    match level {
        Level::Top => flavor.bytes().top,
        Level::Field => flavor.bytes().field,
        Level::Element => flavor::CONTAINED_BYTES,
    }
}

/// The Zod schema checking bytes at `level`.
///
/// `schemas.ts` holds records and nothing else, so [`Level::Top`] never
/// reaches here; it answers with the field spelling anyway, because a type
/// and its schema disagreeing about a position is the one failure this
/// pairing exists to prevent.
pub const fn bytes_schema(flavor: &Flavor, level: Level) -> &'static str {
    match level {
        Level::Top | Level::Field => flavor.bytes().field_schema,
        Level::Element => flavor::CONTAINED_BYTES_SCHEMA,
    }
}

/// napi's automatic `snake_case` -> `camelCase` conversion, applied to every
/// unrenamed function, method, argument, and record field name. The wasm glue
/// spells the same rule (`unibind_backend_wasm::names`), so one JavaScript
/// vocabulary covers both artifacts.
///
/// The rule lives in `unibind_core::casing` because the intra-doc link
/// resolver spells `{@link Machine.forwardPort}` with it: a second copy here
/// is how a link and the member it names would come to disagree.
pub fn camel_case(name: &str) -> String {
    unibind_core::casing::lower_camel_case(name)
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

// Every integer width crosses as a JavaScript `number`, the policy the
// mainstream SDKs (Stripe, OpenAI) ship: a double is exact to +/-2^53, which
// covers every value this platform actually sends -- epoch milliseconds,
// byte counts, microcredits -- and in exchange records are plain JSON
// (`JSON.stringify` works, `JSON.parse` output satisfies the generated Zod
// schemas, timestamps feed `Date`). Both glues refuse an inbound number that
// is fractional or outside the safe-integer range instead of truncating it,
// so the boundary stays checked; see `backend-ts/src/convert.rs` and
// `backend-wasm/src/convert.rs`.

/// The shared refusal for a map the flavor's backend cannot key.
pub fn integer_keyed_map(flavor: &Flavor) -> EmitError {
    EmitError {
        message: format!(
            "integer-keyed maps are not part of the {} backend yet (issue #1993)",
            flavor.target()
        ),
    }
}

/// The TypeScript type of a value crossing at `level`.
///
/// # Errors
///
/// Fails for the surface the compiled glue also rejects (integer-keyed
/// maps, nested streams), so it only trips on IR that never compiled
/// through the matching macro backend.
pub fn ts_type(
    interface: &ir::Interface,
    flavor: &Flavor,
    ty: &ir::Type,
    level: Level,
) -> Result<String, EmitError> {
    Ok(match ty {
        ir::Type::Bool => "boolean".to_owned(),
        // TypeScript has one numeric type, so the IR's integer and float
        // widths both land on `number`. The width is not lost: it is enforced
        // at the boundary by the generated range checks, not by the declared
        // TypeScript type.
        ir::Type::Int(_) | ir::Type::Float(_) => "number".to_owned(),
        ir::Type::String { .. } | ir::Type::Path { .. } => "string".to_owned(),
        ir::Type::Bytes { .. } => bytes_type(flavor, level).to_owned(),
        ir::Type::Option(inner) => {
            format!("{} | null", ts_type(interface, flavor, inner, level)?)
        }
        ir::Type::Vec(inner) => {
            format!(
                "Array<{}>",
                ts_type(interface, flavor, inner, Level::Element)?
            )
        }
        ir::Type::Map { key, value } => {
            if !matches!(**key, ir::Type::String { .. }) {
                return Err(integer_keyed_map(flavor));
            }
            format!(
                "Record<string, {}>",
                ts_type(interface, flavor, value, Level::Element)?
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

/// Resolve a `Named` reference (a record, an enumeration, or an object) to
/// its JavaScript name.
fn named_type_name<'a>(interface: &'a ir::Interface, name: &'a str) -> &'a str {
    if let Some(record) = interface.records.iter().find(|record| record.name == name) {
        return type_name(&record.names, &record.name);
    }
    if let Some(declared) = interface.enums.iter().find(|declared| declared.name == name) {
        return type_name(&declared.names, &declared.name);
    }
    interface
        .objects
        .iter()
        .find(|object| object.name == name)
        .map_or(name, |object| type_name(&object.names, &object.name))
}

/// The union of string literals one enumeration declares:
/// `"running" | "stopped"`.
///
/// A TypeScript `enum` would be the other option and is the wrong one: it is
/// not erasable (so it needs a runtime object where a union needs nothing),
/// its members are not the strings that actually cross, and `JSON.parse`
/// output would not be assignable to it. The value the glue hands back is a
/// plain string, and this is the type that says so exactly.
pub fn literal_union(declared: &ir::Enum) -> String {
    declared
        .variants
        .iter()
        .map(|variant| crate::literal::double_quoted(&variant.wire))
        .collect::<Vec<_>>()
        .join(" | ")
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

/// Whether `index.d.ts` imports node's `Buffer`: the flavor spells one, and
/// the interface has bytes at a position that spells it.
///
/// Record fields count for the node flavor: bytes cross as a `Buffer` there
/// too (see [`Level`]), and a record is declared whether or not any signature
/// mentions it, so a byte field is on its own enough to need the import.
pub fn dts_imports_buffer(flavor: &Flavor, interface: &ir::Interface) -> bool {
    let in_signature = callables(interface).any(|function| {
        function.args.iter().any(|arg| whole_value_bytes(&arg.ty))
            || function.ret.as_ref().is_some_and(whole_value_bytes)
    });
    flavor.bytes().imported && (in_signature || record_field_bytes(interface))
}

/// The same question for `schemas.ts`, which declares records and nothing
/// else: an interface taking a `Buffer` argument but holding no byte field
/// would otherwise get an import nothing uses, which is a type error under
/// `noUnusedLocals`.
pub fn schemas_import_buffer(flavor: &Flavor, interface: &ir::Interface) -> bool {
    flavor.bytes().imported && record_field_bytes(interface)
}

/// Whether any record field carries bytes at field level.
fn record_field_bytes(interface: &ir::Interface) -> bool {
    interface
        .records
        .iter()
        .flat_map(|record| record.fields.iter())
        .any(|field| whole_value_bytes(&field.ty))
}

/// Whether any export returns a stream, which pulls the `UnibindStream`
/// declaration into `index.d.ts` and the `wrapStream` helper into
/// `index.js`. Methods count here: both JavaScript backends render stream
/// methods, where `ir::Interface::has_streams` answers the
/// free-function-only question the backends that reject them ask.
pub fn has_streams(interface: &ir::Interface) -> bool {
    callables(interface).any(|function| matches!(function.ret, Some(ir::Type::Stream(_))))
}

/// Whether `ty`, at the position it was found in, carries the bytes itself:
/// bytes reached through `Option` and `Stream` (which do not change the
/// level), but not bytes inside a `Vec` or a map (which do).
fn whole_value_bytes(ty: &ir::Type) -> bool {
    match ty {
        ir::Type::Bytes { .. } => true,
        ir::Type::Option(inner) | ir::Type::Stream(inner) => whole_value_bytes(inner),
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
