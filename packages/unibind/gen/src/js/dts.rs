//! Render `index.d.ts`: `TSDoc`'d types for every export of the generated
//! package. Fully IR-generated (neither binding library's own `.d.ts`
//! emission is a pipeline this repo runs), so the declared types match what
//! the generated `index.js` wrapper actually exposes: decoded error classes,
//! `AsyncIterable` streams, and object classes with the resource close
//! surface.

use std::fmt::Write as _;

use unibind_core::ir;

use super::flavor::Flavor;
use super::types::{
    self, Level, doc_block, literal_union, resource_close, ts_type, type_name, value_name,
};
use crate::host::EmitError;

/// The one stream shape every stream-returning function shares.
const STREAM_DECL: &str = "\
/**
 * Pull handle over a Rust stream: an `AsyncIterable` that also exposes the
 * raw pull surface. Leaving iteration early (`break`, `return`, `throw`)
 * closes the stream, and the Rust producer sees its stream dropped.
 */
export interface UnibindStream<T> extends AsyncIterable<T> {
  /** The next element, or `null` once the stream ends or closes. */
  next(): Promise<T | null>;
  /** Drop the Rust stream now; a pull already in flight resolves `null`. */
  close(): void;
}

";

/// Render the whole `index.d.ts`.
///
/// # Errors
///
/// Fails for type surface the flavor's backend does not carry (see
/// [`types::ts_type`]).
pub fn render(interface: &ir::Interface, flavor: &Flavor) -> Result<String, EmitError> {
    let mut out = String::new();
    out.push_str(&flavor.banner());
    doc_block(&mut out, "", &interface.docs);
    out.push('\n');
    if let Flavor::Browser { module } = flavor {
        // The initializer is the module's own export, so the module's own
        // declarations type it; re-exporting is all this file owes.
        out.push_str(
            "// `wasm-bindgen`'s initializer, typed by the wasm module's own\n\
             // declarations: a caller awaits it once before calling anything below.\n",
        );
        writeln!(
            out,
            "export {{ default, default as init }} from {};\n",
            crate::literal::double_quoted(module)
        )
        .expect("write to string");
    }
    if types::dts_imports_buffer(flavor, interface) {
        out.push_str("import type { Buffer } from \"node:buffer\";\n\n");
    }
    for declared in &interface.enums {
        enum_decl(&mut out, declared);
    }
    for record in &interface.records {
        record_decl(&mut out, interface, flavor, record)?;
    }
    for error in &interface.errors {
        error_decl(&mut out, error);
    }
    if types::has_streams(interface) {
        out.push_str(STREAM_DECL);
    }
    for object in &interface.objects {
        object_decl(&mut out, interface, flavor, object)?;
    }
    for function in &interface.functions {
        doc_block(&mut out, "", &function.docs);
        out.push_str("export declare function ");
        out.push_str(&callable_signature(interface, flavor, function)?);
        out.push_str(";\n\n");
    }
    let mut trimmed = out.trim_end().to_owned();
    trimmed.push('\n');
    Ok(trimmed)
}

/// One enumeration: a union of the string literals that actually cross.
///
/// `export type` rather than `export enum`: the value the glue hands back is
/// a plain string, a union is erased at compile time, and `JSON.parse` output
/// is assignable to it. A TypeScript `enum` would be none of those things.
fn enum_decl(out: &mut String, declared: &ir::Enum) {
    // Per-variant docs have nowhere of their own to live in a union, so they
    // join the type's doc block as a list. Two adjacent JSDoc blocks would
    // not work: an editor reads only the one nearest the declaration, so the
    // variant meanings would be written down and still invisible.
    let mut docs = declared.docs.clone();
    let documented = declared
        .variants
        .iter()
        .filter(|variant| !variant.docs.is_empty());
    for (at, variant) in documented.enumerate() {
        if at == 0 && !docs.is_empty() {
            docs.push(String::new());
        }
        docs.push(format!(
            "- `{}`: {}",
            variant.wire,
            variant.docs.join(" ").trim()
        ));
    }
    doc_block(out, "", &docs);
    writeln!(
        out,
        "export type {} = {};\n",
        type_name(&declared.names, &declared.name),
        literal_union(declared)
    )
    .expect("write to string");
}

fn record_decl(
    out: &mut String,
    interface: &ir::Interface,
    flavor: &Flavor,
    record: &ir::Record,
) -> Result<(), EmitError> {
    doc_block(out, "", &record.docs);
    out.push_str("export interface ");
    out.push_str(type_name(&record.names, &record.name));
    out.push_str(" {\n");
    for field in &record.fields {
        doc_block(out, "  ", &field.docs);
        let name = value_name(&field.name, &field.names);
        let entry = match &field.ty {
            // Both glues read a missing property (or an explicit null) as
            // `None`, and hand `None` back as an absent value rather than an
            // explicit null, which the conformance suite pins. So the
            // declaration needs both halves: `?` for what comes back, and
            // `| null` for what a caller may pass in.
            ir::Type::Option(inner) => {
                format!(
                    "readonly {name}?: {} | null",
                    ts_type(interface, flavor, inner, Level::Field)?
                )
            }
            ty => format!(
                "readonly {name}: {}",
                ts_type(interface, flavor, ty, Level::Field)?
            ),
        };
        writeln!(out, "  {entry};").expect("write to string");
    }
    out.push_str("}\n\n");
    Ok(())
}

/// One base class per error enum plus one subclass per variant, mirroring
/// the classes `index.js` defines and its decoder throws.
fn error_decl(out: &mut String, error: &ir::ErrorType) {
    let base = type_name(&error.names, &error.name);
    doc_block(out, "", &error.docs);
    writeln!(out, "export declare class {base} extends Error {{").expect("write to string");
    out.push_str("  /** The variant's class name: which subclass this instance is. */\n");
    out.push_str("  code: string;\n}\n");
    for variant in &error.variants {
        doc_block(out, "", &variant.docs);
        writeln!(
            out,
            "export declare class {} extends {base} {{}}",
            type_name(&variant.names, &variant.name)
        )
        .expect("write to string");
    }
    out.push('\n');
}

fn object_decl(
    out: &mut String,
    interface: &ir::Interface,
    flavor: &Flavor,
    object: &ir::Object,
) -> Result<(), EmitError> {
    doc_block(out, "", &object.docs);
    writeln!(
        out,
        "export declare class {} {{",
        type_name(&object.names, &object.name)
    )
    .expect("write to string");
    if let Some(ctor) = &object.constructor {
        doc_block(out, "  ", &ctor.docs);
        writeln!(out, "  constructor({});", params_list(interface, flavor, ctor)?)
            .expect("write to string");
    } else {
        out.push_str("  /** Instances come from the exported functions returning this type. */\n");
        out.push_str("  private constructor();\n");
    }
    for factory in &object.associated {
        doc_block(out, "  ", &factory.docs);
        writeln!(
            out,
            "  static {};",
            callable_signature(interface, flavor, factory)?
        )
        .expect("write to string");
    }
    let close = resource_close(object);
    for method in &object.methods {
        if close.is_some_and(|close| std::ptr::eq(close, method)) {
            continue;
        }
        doc_block(out, "  ", &method.docs);
        writeln!(out, "  {};", callable_signature(interface, flavor, method)?)
            .expect("write to string");
    }
    if let Some(close) = close {
        doc_block(out, "  ", &close.docs);
        let ret = match close.asyncness {
            ir::Asyncness::Async => "Promise<void>",
            ir::Asyncness::Sync => "void",
        };
        writeln!(out, "  close(): {ret};").expect("write to string");
        out.push_str("  /** `await using` support: closes the resource. */\n");
        out.push_str("  [Symbol.asyncDispose](): Promise<void>;\n");
    }
    out.push_str("}\n\n");
    Ok(())
}

/// The comma-joined parameter list: defaulted and `Option` arguments are
/// optional, async callables gain the trailing `signal?: AbortSignal`.
fn params_list(
    interface: &ir::Interface,
    flavor: &Flavor,
    function: &ir::Function,
) -> Result<String, EmitError> {
    let mut params = Vec::new();
    for arg in &function.args {
        let name = value_name(&arg.name, &arg.names);
        params.push(match &arg.ty {
            ir::Type::Option(inner) => {
                format!(
                    "{name}?: {} | null",
                    ts_type(interface, flavor, inner, Level::Top)?
                )
            }
            ty if arg.default.is_some() => {
                format!("{name}?: {}", ts_type(interface, flavor, ty, Level::Top)?)
            }
            ty => format!("{name}: {}", ts_type(interface, flavor, ty, Level::Top)?),
        });
    }
    if matches!(function.asyncness, ir::Asyncness::Async) {
        params.push("signal?: AbortSignal".to_owned());
    }
    Ok(params.join(", "))
}

/// `name(params): ret` for a function or method; async callables return a
/// `Promise`, streams return the shared `UnibindStream<T>` shape.
fn callable_signature(
    interface: &ir::Interface,
    flavor: &Flavor,
    function: &ir::Function,
) -> Result<String, EmitError> {
    let params = params_list(interface, flavor, function)?;
    let ret = match &function.ret {
        None => "void".to_owned(),
        Some(ir::Type::Stream(element)) => {
            format!(
                "UnibindStream<{}>",
                ts_type(interface, flavor, element, Level::Top)?
            )
        }
        Some(ty) => ts_type(interface, flavor, ty, Level::Top)?,
    };
    let ret = if matches!(function.asyncness, ir::Asyncness::Async) {
        format!("Promise<{ret}>")
    } else {
        ret
    };
    Ok(format!(
        "{}({params}): {ret}",
        value_name(&function.name, &function.names)
    ))
}
