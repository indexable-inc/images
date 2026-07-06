//! The public static methods of the module class, their trailing-default
//! overloads, and the status-to-exception mapping helpers.

use unibind_core::ir;

use crate::ctype::{CTy, EnvelopeLayout};
use crate::java::{decode, encode, line, types};
use crate::java::overloads::overloads;
use crate::model::Model;
use crate::{names, RenderError};

/// Every public method (with overloads) followed by the error helpers.
pub fn all(interface: &ir::Interface, model: &Model<'_>) -> Result<String, RenderError> {
    let mut out = String::new();
    for function in &interface.functions {
        out.push('\n');
        out.push_str(&method(function, model));
        for overload in overloads(function)? {
            out.push('\n');
            out.push_str(&overload);
        }
    }
    out.push_str(&error_helpers(interface));
    Ok(out)
}

fn method(function: &ir::Function, model: &Model<'_>) -> String {
    let name = names::camel(&function.name);
    let ret = function.ret.as_ref().map(CTy::of);
    let envelope = model.envelope(ret.as_ref());

    let mut out = types::doc_block(&method_docs(function), 1);
    line(
        &mut out,
        1,
        &format!("public static {} {name}({}) {{", ret_type(ret.as_ref()), params(function)),
    );

    let has_aggregate = function
        .args
        .iter()
        .any(|arg| !CTy::of(&arg.ty).is_scalar());
    let base = if has_aggregate {
        line(&mut out, 2, "try (Arena arena = Arena.ofConfined()) {");
        3
    } else {
        2
    };

    let mut call_args = Vec::new();
    for arg in &function.args {
        let cty = CTy::of(&arg.ty);
        let camel = names::camel(&arg.name);
        if cty.is_scalar() {
            call_args.push(types::downcall_arg(&cty, &camel));
        } else {
            let layout = model.layout(&cty);
            let segment = format!("{camel}Arg");
            line(
                &mut out,
                base,
                &format!("MemorySegment {segment} = arena.allocate({}, {});", layout.size, layout.align),
            );
            line(&mut out, base, &format!("{};", encode::write_stmt(&cty, &segment, "0", &camel)));
            call_args.push(segment);
        }
    }

    let site = CallSite {
        function,
        envelope: &envelope,
        ret: ret.as_ref(),
        call_args: call_args.join(", "),
    };
    invoke_and_decode(&mut out, base, &site);
    if has_aggregate {
        line(&mut out, 2, "}");
    }
    line(&mut out, 1, "}");
    out
}

/// Docs plus per-parameter notes for one method.
fn method_docs(function: &ir::Function) -> Vec<String> {
    let mut doc = function.docs.clone();
    let notes: Vec<String> = function
        .args
        .iter()
        .filter_map(|arg| {
            let notes = types::doc_notes(&CTy::of(&arg.ty));
            if notes.is_empty() {
                None
            } else {
                Some(format!("@param {} {}", names::camel(&arg.name), notes.join(" ")))
            }
        })
        .collect();
    if !notes.is_empty() {
        if !doc.is_empty() {
            doc.push(String::new());
        }
        doc.extend(notes);
    }
    doc
}

/// The downcall a public method makes.
struct CallSite<'a> {
    function: &'a ir::Function,
    envelope: &'a EnvelopeLayout,
    ret: Option<&'a CTy>,
    call_args: String,
}

/// Invoke the handle, map non-zero status to an exception, decode the
/// value, and free the envelope.
fn invoke_and_decode(out: &mut String, base: usize, site: &CallSite<'_>) {
    let handle = names::handle_const(&site.function.name);
    line(&mut *out, base, "MemorySegment envelope;");
    line(&mut *out, base, "try {");
    line(
        &mut *out,
        base + 1,
        &format!("envelope = (MemorySegment) {handle}.invokeExact({});", site.call_args),
    );
    line(&mut *out, base, "} catch (Throwable error) {");
    line(
        &mut *out,
        base + 1,
        &format!(
            "throw new IllegalStateException(\"unibind downcall {} failed\", error);",
            site.function.name
        ),
    );
    line(&mut *out, base, "}");
    line(&mut *out, base, &format!("envelope = envelope.reinterpret({});", site.envelope.layout.size));
    line(&mut *out, base, "try {");
    line(&mut *out, base + 1, "int code = envelope.get(ValueLayout.JAVA_INT, 0);");
    line(&mut *out, base + 1, "if (code != 0) {");
    let raise = site.function.throws.as_ref().map_or_else(
        || "unexpectedStatus".to_owned(),
        |throws| format!("{}Exception", names::decapitalize(throws)),
    );
    line(
        &mut *out,
        base + 2,
        &format!(
            "throw {raise}(code, decodeStr(envelope, {}));",
            site.envelope.err_msg_offset
        ),
    );
    line(&mut *out, base + 1, "}");
    if let Some(ret) = site.ret {
        let value_offset = site
            .envelope
            .value_offset
            .expect("a function with a return type has a value slot");
        let read = decode::read_expr(ret, "envelope", &value_offset.to_string());
        line(&mut *out, base + 1, &format!("return {read};"));
    }
    line(&mut *out, base, "} finally {");
    line(&mut *out, base + 1, &format!("free({handle}_FREE, envelope);"));
    line(&mut *out, base, "}");
}

pub(super) fn ret_type(ret: Option<&CTy>) -> String {
    ret.map_or_else(|| "void".to_owned(), |ty| types::java_type(ty, false))
}

fn params(function: &ir::Function) -> String {
    let rendered: Vec<String> = function
        .args
        .iter()
        .map(|arg| {
            format!(
                "{} {}",
                types::java_type(&CTy::of(&arg.ty), false),
                names::camel(&arg.name)
            )
        })
        .collect();
    rendered.join(", ")
}

fn error_helpers(interface: &ir::Interface) -> String {
    let mut out = String::new();
    for error in &interface.errors {
        out.push('\n');
        line(
            &mut out,
            1,
            &format!(
                "private static RuntimeException {}Exception(int code, String message) {{",
                names::decapitalize(&error.name)
            ),
        );
        line(&mut out, 2, "return switch (code) {");
        for (index, variant) in error.variants.iter().enumerate() {
            line(
                &mut out,
                3,
                &format!(
                    "case {} -> new {}Exception.{}(message);",
                    index + 1,
                    error.name,
                    variant.name
                ),
            );
        }
        line(&mut out, 3, "default -> unexpectedStatus(code, message);");
        line(&mut out, 2, "};");
        line(&mut out, 1, "}");
    }
    out.push('\n');
    line(
        &mut out,
        1,
        "private static RuntimeException unexpectedStatus(int code, String message) {",
    );
    line(&mut out, 2, "if (code == -1) {");
    line(&mut out, 3, "return new UnibindPanicException(message);");
    line(&mut out, 2, "}");
    line(
        &mut out,
        2,
        "return new IllegalStateException(\"unexpected unibind status \" + code + \": \" + message);",
    );
    line(&mut out, 1, "}");
    out
}
