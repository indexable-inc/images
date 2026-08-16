//! Render `schemas.ts`: one [Zod](https://zod.dev) schema per record, so a
//! consumer can validate a value crossing the boundary at runtime, plus one
//! `z.enum` per enumeration.
//!
//! The schemas come out of the same IR as `index.d.ts` through the same type
//! mapping ([`super::types`]), so a schema cannot drift from the declared
//! type: both files are regenerated from the artifact that shipped. Only
//! records and enumerations appear here. Errors are classes `index.js`
//! throws, objects are handles that cross by reference, and streams are a
//! function's whole return type -- none of them is a value with a data shape
//! to check.
//!
//! Doc comments ride along as `.describe(...)` rather than `TSDoc`: the
//! description survives into the schema at runtime (and into a JSON Schema
//! conversion), which a comment above the `const` would not.

use std::collections::HashSet;
use std::fmt::Write as _;

use unibind_core::ir;

use super::flavor::Flavor;
use super::types::{self, Level, bytes_schema, doc_block, type_name, value_name};
use crate::host::EmitError;

/// Render the whole `schemas.ts`.
///
/// # Errors
///
/// Fails for the type surface `index.d.ts` also refuses (integer-keyed maps;
/// see [`types::ts_type`]), for a record graph with a reference cycle, and
/// for a record field naming something other than a record.
pub fn render(interface: &ir::Interface, flavor: &Flavor) -> Result<String, EmitError> {
    reject_reference_cycles(interface)?;
    let mut out = String::new();
    out.push_str(&flavor.banner());
    doc_block(&mut out, "", &interface.docs);
    out.push('\n');
    // A value import, not a type-only one: `z.instanceof(Buffer)` reads the
    // constructor at run time.
    if types::schemas_import_buffer(flavor, interface) {
        out.push_str("import { Buffer } from \"node:buffer\";\n");
    }
    out.push_str("import { z } from \"zod\";\n\n");
    // Enumerations first: a record's field schema reads one by name, and a
    // `const` is not hoisted, so the reference has to be already bound.
    for declared in &interface.enums {
        enum_schema(&mut out, declared);
    }
    for (at, record) in interface.records.iter().enumerate() {
        record_schema(
            &mut out,
            &Scope {
                interface,
                flavor,
                at,
            },
            record,
        )?;
    }
    let mut trimmed = out.trim_end().to_owned();
    trimmed.push('\n');
    Ok(trimmed)
}

/// Where the record being rendered sits: the interface it belongs to, the
/// flavor being rendered, and its own position in declaration order, which is
/// what decides whether a reference to another record has to defer through
/// `z.lazy`.
struct Scope<'a> {
    interface: &'a ir::Interface,
    flavor: &'a Flavor,
    at: usize,
}

/// One `export const <Record> = z.object({...})` plus the `z.infer` type
/// re-export, so a consumer imports the schema and the type under one name.
fn record_schema(
    out: &mut String,
    scope: &Scope<'_>,
    record: &ir::Record,
) -> Result<(), EmitError> {
    let name = type_name(&record.names, &record.name);
    writeln!(out, "export const {name} = z").expect("write to string");
    out.push_str("  .object({\n");
    for field in &record.fields {
        let key = value_name(&field.name, &field.names);
        let schema = field_schema(scope, &field.ty)?;
        let entry = describe_argument(&field.docs).map_or_else(
            || format!("{key}: {schema}"),
            |description| format!("{key}: {schema}.describe({description})"),
        );
        writeln!(out, "    {entry},").expect("write to string");
    }
    let tail = describe_argument(&record.docs).map_or_else(
        || ";".to_owned(),
        |description| format!("\n  .describe({description});"),
    );
    writeln!(out, "  }}){tail}").expect("write to string");
    writeln!(out, "export type {name} = z.infer<typeof {name}>;\n").expect("write to string");
    Ok(())
}

/// The schema of a record field. An `Option` field is both nullable and
/// optional, matching the `name?: T | null` the `.d.ts` declares: the glue
/// reads a missing property as `None` and writes `None` back as an absent
/// value.
fn field_schema(scope: &Scope<'_>, ty: &ir::Type) -> Result<String, EmitError> {
    match ty {
        ir::Type::Option(inner) => Ok(format!(
            "{}.nullable().optional()",
            zod_type(scope, inner, Level::Field)?
        )),
        ty => zod_type(scope, ty, Level::Field),
    }
}

/// The Zod schema of a value at `level`, mirroring [`types::ts_type`] type
/// for type and position for position: a field's bytes check for exactly what
/// the `.d.ts` declares there, and a container's interior checks for the
/// array of numbers the `.d.ts` declares inside it.
fn zod_type(scope: &Scope<'_>, ty: &ir::Type, level: Level) -> Result<String, EmitError> {
    Ok(match ty {
        ir::Type::Bool => "z.boolean()".to_owned(),
        // Every integer width is a `number` in the `.d.ts`, so every integer
        // schema is `z.number().int()`; JSON round trips stay checkable.
        ir::Type::Int(_) => "z.number().int()".to_owned(),
        ir::Type::Float(_) => "z.number()".to_owned(),
        ir::Type::String { .. } | ir::Type::Path { .. } => "z.string()".to_owned(),
        // Whichever the flavor spells: `z.instanceof` is the honest check for
        // a node `Buffer`, because the value that crosses is a real one and
        // nothing weaker (a length check, a number array) would tell it from
        // the array a container's bytes cross as. A browser has no `Buffer`,
        // and serde carries a twin's bytes as that same array, so there the
        // two positions check the same way. `z.infer` reads either back as
        // the type the `.d.ts` declares.
        ir::Type::Bytes { .. } => bytes_schema(scope.flavor, level).to_owned(),
        ir::Type::Option(inner) => format!("{}.nullable()", zod_type(scope, inner, level)?),
        ir::Type::Vec(inner) => format!("z.array({})", zod_type(scope, inner, Level::Element)?),
        ir::Type::Map { key, value } => {
            if !matches!(**key, ir::Type::String { .. }) {
                return Err(types::integer_keyed_map(scope.flavor));
            }
            // The two-argument spelling: the one `z.record` call that means
            // the same thing on zod 3 and zod 4.
            format!(
                "z.record(z.string(), {})",
                zod_type(scope, value, Level::Element)?
            )
        }
        ir::Type::Named(name) => named_schema(scope, name)?,
        ir::Type::Stream(_) => {
            return Err(EmitError {
                message: "streams cross only as a whole function return type".to_owned(),
            });
        }
    })
}

/// One `export const <Enum> = z.enum([...])` plus the `z.infer` type, the
/// same pairing every record schema uses, so a consumer imports one name and
/// gets both the checker and the type.
///
/// `z.enum` over the wire strings rather than `z.nativeEnum`: what crosses is
/// a plain string, and `z.infer` reads a `z.enum` back as exactly the union
/// `index.d.ts` declares.
fn enum_schema(out: &mut String, declared: &ir::Enum) {
    let name = type_name(&declared.names, &declared.name);
    let members = declared
        .variants
        .iter()
        .map(|variant| crate::literal::double_quoted(&variant.wire))
        .collect::<Vec<_>>()
        .join(", ");
    let tail = describe_argument(&declared.docs).map_or_else(
        || ";".to_owned(),
        |description| format!(".describe({description});"),
    );
    writeln!(out, "export const {name} = z.enum([{members}]){tail}").expect("write to string");
    writeln!(out, "export type {name} = z.infer<typeof {name}>;\n").expect("write to string");
}

/// The schema a `Named` field reads, deferred when it is not bound yet.
fn named_schema(scope: &Scope<'_>, name: &str) -> Result<String, EmitError> {
    // Enumerations are all emitted above every record, so a reference to one
    // is always bound and never needs the `z.lazy` thunk.
    if let Some(declared) = scope
        .interface
        .enums
        .iter()
        .find(|declared| declared.name == name)
    {
        return Ok(type_name(&declared.names, &declared.name).to_owned());
    }
    let Some((at, record)) = scope
        .interface
        .records
        .iter()
        .enumerate()
        .find(|(_, record)| record.name == name)
    else {
        return Err(EmitError {
            message: format!(
                "`{name}` is not a record or enumeration in this interface, so \
                 it has no Zod schema; only records and enumerations cross by \
                 value (an object handle crosses by reference, and its fields \
                 never leave Rust)"
            ),
        });
    };
    let schema = type_name(&record.names, &record.name);
    // A `const` is not hoisted, so a record declared later is still in its
    // temporal dead zone when this one initializes; the thunk reads it after
    // the whole module has evaluated. Cycles are refused before this runs, so
    // a reference to an earlier record is always bound.
    if at > scope.at {
        Ok(format!("z.lazy(() => {schema})"))
    } else {
        Ok(schema.to_owned())
    }
}

/// Refuse a record graph whose references form a cycle.
///
/// Every schema is a `const` initialized from the schemas it references, and
/// `z.infer` on a self-referential schema has no type to infer, so a cyclic
/// graph has no `schemas.ts` that both evaluates and type-checks. Lowering
/// never produces one today (a record field is owned data, and the only way
/// back to the same record is through `Vec`/`Option`/`HashMap`), so this
/// refusal names the cycle rather than silently emitting a broken file.
fn reject_reference_cycles(interface: &ir::Interface) -> Result<(), EmitError> {
    let mut settled = HashSet::new();
    let mut path = Vec::new();
    for record in &interface.records {
        visit(interface, record, &mut path, &mut settled)?;
    }
    Ok(())
}

fn visit<'a>(
    interface: &'a ir::Interface,
    record: &'a ir::Record,
    path: &mut Vec<&'a str>,
    settled: &mut HashSet<&'a str>,
) -> Result<(), EmitError> {
    if settled.contains(record.name.as_str()) {
        return Ok(());
    }
    if let Some(start) = path.iter().position(|name| *name == record.name) {
        let mut cycle = path[start..].to_vec();
        cycle.push(record.name.as_str());
        return Err(EmitError {
            message: format!(
                "the records {} form a reference cycle; a Zod schema reads the \
                 schemas it references while it initializes, so `schemas.ts` \
                 needs an acyclic record graph",
                cycle.join(" -> ")
            ),
        });
    }
    path.push(record.name.as_str());
    let mut referenced = Vec::new();
    for field in &record.fields {
        // The annotation pins the leaf to the interface's lifetime, so the
        // names it gathers outlive the walk.
        field.ty.for_each_leaf(&mut |leaf: &'a ir::Type| {
            if let ir::Type::Named(name) = leaf {
                referenced.push(name.as_str());
            }
        });
    }
    for name in referenced {
        // A name that is not a record refuses when it renders, with a message
        // about what does and does not cross by value.
        if let Some(next) = interface.records.iter().find(|record| record.name == *name) {
            visit(interface, next, path, settled)?;
        }
    }
    path.pop();
    settled.insert(record.name.as_str());
    Ok(())
}

/// The doc comment as the string literal `.describe` takes, or `None` when
/// the item has none. A description is one prose string, so each line is
/// trimmed (the leading space every `///` line carries included) and the
/// blank lines survive as the `\n\n` paragraph breaks the `.d.ts` doc block
/// renders.
fn describe_argument(docs: &[String]) -> Option<String> {
    if docs.is_empty() {
        return None;
    }
    let text = docs
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n");
    Some(crate::literal::double_quoted(text.trim()))
}
