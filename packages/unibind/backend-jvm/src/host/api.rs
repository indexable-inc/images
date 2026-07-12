//! The public static methods: encode the arguments, call through the
//! method handle, decode the reply envelope, and rebuild errors as
//! exceptions. Trailing defaulted arguments also get delegating overloads.

use std::fmt::Write as _;

use heck::ToLowerCamelCase as _;
use unibind_core::ir;

use super::java;
use crate::{names, RenderError};

/// Render every public method, primaries first, then their
/// trailing-default overloads.
pub fn render(interface: &ir::Interface) -> Result<String, RenderError> {
    let mut methods = Vec::new();
    for function in &interface.functions {
        methods.push(primary(function, interface)?);
        methods.extend(overloads(function, interface)?);
    }
    Ok(methods.join("\n\n"))
}

/// The full-signature method carrying one exported function.
fn primary(function: &ir::Function, interface: &ir::Interface) -> Result<String, RenderError> {
    let method = names::method_name(function)?;
    let handle = names::handle_constant(function);
    let params = params(function, interface)?;
    let signature = params
        .iter()
        .map(|Param { ty, name }| format!("{ty} {name}"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut body = String::from("UnibindWire args = new UnibindWire();\n");
    for (arg, param) in function.args.iter().zip(&params) {
        body.push_str(&java::encode(&arg.ty, interface, &param.name, "args", 0));
        body.push('\n');
    }
    let _ = writeln!(body, "UnibindReader reply = call({handle}, args);");
    body.push_str("int status = Byte.toUnsignedInt(reply.readByte());\n");
    if let Some(throws) = &function.throws {
        let factory = factory_name(interface, throws);
        body.push_str("if (status == 1) {\n");
        body.push_str("    int variant = reply.readInt();\n");
        body.push_str("    String message = reply.readString();\n");
        body.push_str("    reply.finish();\n");
        let _ = writeln!(body, "    throw {factory}(variant, message);");
        body.push_str("}\n");
    }
    body.push_str("expectOk(status, reply);\n");
    match &function.ret {
        None => body.push_str("reply.finish();"),
        Some(ret) => {
            let declared = java::declared(ret, interface);
            let decode = java::decode(ret, interface, "reply", 0);
            let _ = writeln!(body, "{declared} result = {decode};");
            body.push_str("reply.finish();\nreturn result;");
        }
    }

    let ret = function
        .ret
        .as_ref()
        .map_or_else(|| "void".to_owned(), |ret| java::declared(ret, interface));
    let throws = throws_clause(function, interface);
    Ok(format!(
        "{javadoc}public static {ret} {method}({signature}){throws} {{\n{body}\n}}",
        javadoc = primary_javadoc(function, interface),
        body = java::indent(&body, 1),
    ))
}

/// One delegating overload per length of the trailing defaulted-argument
/// run: dropping the last `k` defaulted arguments calls the primary with
/// their default literals.
fn overloads(
    function: &ir::Function,
    interface: &ir::Interface,
) -> Result<Vec<String>, RenderError> {
    let trailing = function
        .args
        .iter()
        .rev()
        .take_while(|arg| defaulted(arg))
        .count();
    let method = names::method_name(function)?;
    let all = params(function, interface)?;
    let throws = throws_clause(function, interface);
    let ret = function
        .ret
        .as_ref()
        .map_or_else(|| "void".to_owned(), |ret| java::declared(ret, interface));
    let raw_types = all
        .iter()
        .map(|param| java::strip_generics(&param.ty))
        .collect::<Vec<_>>()
        .join(", ");

    let mut rendered = Vec::new();
    for dropped in 1..=trailing {
        let kept = function.args.len() - dropped;
        let signature = all[..kept]
            .iter()
            .map(|Param { ty, name }| format!("{ty} {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut forwarded: Vec<String> = all[..kept]
            .iter()
            .map(|param| param.name.clone())
            .collect();
        for arg in &function.args[kept..] {
            forwarded.push(default_literal(arg)?);
        }
        let call = format!("{method}({});", forwarded.join(", "));
        let body = if function.ret.is_some() {
            format!("return {call}")
        } else {
            call
        };
        let names = function.args[kept..]
            .iter()
            .map(|arg| Ok(java::code(&names::arg_name(arg)?)))
            .collect::<Result<Vec<_>, RenderError>>()?
            .join(", ");
        let javadoc = java::javadoc(&[format!(
            "Calls {{@link #{method}({raw_types})}} with {names} defaulted."
        )]);
        rendered.push(format!(
            "{javadoc}public static {ret} {method}({signature}){throws} {{\n    {body}\n}}"
        ));
    }
    Ok(rendered)
}

/// Whether dropping this argument from an overload has a value to pass:
/// an explicit `default = ...`, or an `Option` (which defaults to `None`).
const fn defaulted(arg: &ir::Arg) -> bool {
    arg.default.is_some() || matches!(arg.ty, ir::Type::Option(_))
}

/// The Java literal an overload passes for a dropped argument.
fn default_literal(arg: &ir::Arg) -> Result<String, RenderError> {
    // `Option` arguments without an explicit default get `None`.
    let literal = arg.default.as_ref().unwrap_or(&ir::Literal::None);
    // A default on `Option<T>` describes the payload type `T`.
    let target = match &arg.ty {
        ir::Type::Option(inner) => inner,
        other => other,
    };
    let spelled = match (literal, target) {
        (ir::Literal::None, _) => "null".to_owned(),
        (ir::Literal::Bool(value), ir::Type::Bool) => value.to_string(),
        (ir::Literal::Int(value), ir::Type::Int(kind)) => int_literal(*value, *kind),
        (ir::Literal::Int(value), ir::Type::Float(ir::FloatKind::F32)) => format!("{value}.0f"),
        (ir::Literal::Int(value), ir::Type::Float(ir::FloatKind::F64)) => format!("{value}.0"),
        (ir::Literal::Float(value), ir::Type::Float(kind)) => float_literal(*value, *kind),
        (ir::Literal::Str(value), ir::Type::String { .. }) => java::string_literal(value),
        (ir::Literal::Str(value), ir::Type::Path { .. }) => {
            format!("Path.of({})", java::string_literal(value))
        }
        _ => {
            return Err(RenderError::new(format!(
                "argument `{}` has a default the jvm backend cannot spell \
                 for its type",
                arg.name
            )));
        }
    };
    Ok(spelled)
}

/// An integer literal at the argument's Java primitive width.
fn int_literal(value: i64, kind: ir::IntKind) -> String {
    match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => format!("(byte) {value}"),
        ir::IntKind::I16 | ir::IntKind::U16 => format!("(short) {value}"),
        ir::IntKind::I32 | ir::IntKind::U32 => format!("{value}"),
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize => {
            format!("{value}L")
        }
    }
}

/// A float literal, spelling the non-finite values Java has no literal for.
fn float_literal(value: f64, kind: ir::FloatKind) -> String {
    let class = match kind {
        ir::FloatKind::F32 => "Float",
        ir::FloatKind::F64 => "Double",
    };
    if value.is_nan() {
        return format!("{class}.NaN");
    }
    if value.is_infinite() {
        let sign = if value > 0.0 { "POSITIVE" } else { "NEGATIVE" };
        return format!("{class}.{sign}_INFINITY");
    }
    match kind {
        ir::FloatKind::F32 => format!("{value:?}f"),
        ir::FloatKind::F64 => format!("{value:?}"),
    }
}

/// A declared Java parameter: its type and name. Named (rather than a bare
/// tuple) to satisfy the workspace's `clippy::anonymous_tuple_return_type`.
struct Param {
    ty: String,
    name: String,
}

/// The declared Java parameter list, one [`Param`] per argument.
fn params(function: &ir::Function, interface: &ir::Interface) -> Result<Vec<Param>, RenderError> {
    function
        .args
        .iter()
        .map(|arg| {
            Ok(Param {
                ty: java::declared(&arg.ty, interface),
                name: names::arg_name(arg)?,
            })
        })
        .collect()
}

/// The ` throws X` clause, present whenever the function declares an error.
fn throws_clause(function: &ir::Function, interface: &ir::Interface) -> String {
    function.throws.as_ref().map_or_else(String::new, |name| {
        format!(" throws {}", names::exception_name_of(interface, name))
    })
}

/// The private factory rebuilding one error's exception from the wire.
fn factory_name(interface: &ir::Interface, error_name: &str) -> String {
    names::exception_name_of(interface, error_name).to_lower_camel_case()
}

/// The primary method's javadoc: the doc comment plus a `@throws` line.
fn primary_javadoc(function: &ir::Function, interface: &ir::Interface) -> String {
    let mut lines: Vec<String> = function
        .docs
        .iter()
        .map(|line| java::javadoc_escape(line.trim()))
        .collect();
    if let Some(throws) = &function.throws {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(format!(
            "@throws {} on the failure the native side reports",
            names::exception_name_of(interface, throws),
        ));
    }
    java::javadoc(&lines)
}
