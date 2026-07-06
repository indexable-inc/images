//! Generate the Kotlin sugar layer: top-level functions with real default
//! parameter values delegating to the generated Java class. Same package,
//! same types, no second FFI path.

use std::fmt::Write as _;

use unibind_core::ir;

use crate::ctype::CTy;
use crate::java::types;
use crate::model::Model;
use crate::{names, RenderError, SourceFile};

/// Generate the Kotlin source for one interface: a single `<Module>.kt`
/// next to the Java sources.
///
/// # Errors
///
/// Fails for surface the sync JVM backend does not implement (async
/// functions, data enums, objects) and for defaults that do not fit their
/// parameter type.
pub fn generate_kotlin(interface: &ir::Interface) -> Result<Vec<SourceFile>, RenderError> {
    // Validation only: the Kotlin layer reuses the Java type surface.
    Model::new(interface)?;
    let class = names::pascal(&interface.name);
    let mut out = format!(
        "// Kotlin sugar over the Java Panama binding [{class}]; no second FFI path.\n\
         // suspend/Flow sugar lands with async IR (#2083 follow-up)\n\
         package {}\n",
        names::java_package(&interface.name)
    );
    for function in &interface.functions {
        out.push('\n');
        out.push_str(&render_fn(function, &class)?);
    }
    Ok(vec![SourceFile {
        path: format!("unibind/{}/{class}.kt", interface.name),
        content: out,
    }])
}

fn render_fn(function: &ir::Function, class: &str) -> Result<String, RenderError> {
    let name = names::camel(&function.name);

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

    let mut out = types::doc_block(&doc, 0);
    let mut params = Vec::new();
    let mut forwarded = Vec::new();
    for arg in &function.args {
        let cty = CTy::of(&arg.ty);
        let camel = names::camel(&arg.name);
        let mut param = format!("    {camel}: {}", kotlin_type(&cty));
        let default = match &arg.default {
            Some(literal) => Some(kotlin_literal(&cty, literal)?),
            None if matches!(cty, CTy::Option(_)) => Some("null".to_owned()),
            None => None,
        };
        if let Some(default) = default {
            let _ = write!(param, " = {default}");
        }
        params.push(param);
        forwarded.push(camel);
    }

    let ret = function
        .ret
        .as_ref()
        .map_or_else(|| "Unit".to_owned(), |ty| kotlin_type(&CTy::of(ty)));
    if params.is_empty() {
        let _ = writeln!(out, "fun {name}(): {ret} =");
    } else {
        let _ = writeln!(out, "fun {name}(\n{},\n): {ret} =", params.join(",\n"));
    }
    let _ = writeln!(out, "    {class}.{name}({})", forwarded.join(", "));
    Ok(out)
}

fn kotlin_type(ty: &CTy) -> String {
    match ty {
        CTy::Bool => "Boolean".to_owned(),
        CTy::Int(kind) => int_type(*kind).to_owned(),
        CTy::Float(ir::FloatKind::F32) => "Float".to_owned(),
        CTy::Float(ir::FloatKind::F64) => "Double".to_owned(),
        CTy::Str => "String".to_owned(),
        CTy::Path => "java.nio.file.Path".to_owned(),
        CTy::Bytes => "ByteArray".to_owned(),
        CTy::Option(inner) => format!("{}?", kotlin_type(inner)),
        CTy::Vec(inner) => format!("List<{}>", kotlin_type(inner)),
        CTy::Map { key, value } => format!("Map<{}, {}>", kotlin_type(key), kotlin_type(value)),
        CTy::Record(name) => name.clone(),
    }
}

const fn int_type(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => "Byte",
        ir::IntKind::I16 | ir::IntKind::U16 => "Short",
        ir::IntKind::I32 | ir::IntKind::U32 => "Int",
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize => "Long",
    }
}

fn kotlin_literal(ty: &CTy, literal: &ir::Literal) -> Result<String, RenderError> {
    match (ty, literal) {
        (CTy::Option(_), ir::Literal::None) => Ok("null".to_owned()),
        (CTy::Option(inner), _) => kotlin_literal(inner, literal),
        (CTy::Bool, ir::Literal::Bool(value)) => Ok(value.to_string()),
        (CTy::Int(_), ir::Literal::Int(value)) => Ok(value.to_string()),
        (CTy::Float(ir::FloatKind::F32), ir::Literal::Int(value)) => Ok(format!("{value}.0f")),
        (CTy::Float(ir::FloatKind::F64), ir::Literal::Int(value)) => Ok(format!("{value}.0")),
        (CTy::Float(ir::FloatKind::F32), ir::Literal::Float(value)) => Ok(format!("{value:?}f")),
        (CTy::Float(ir::FloatKind::F64), ir::Literal::Float(value)) => Ok(format!("{value:?}")),
        (CTy::Str, ir::Literal::Str(value)) => Ok(types::quoted(value, true)),
        (ty, literal) => Err(RenderError::new(format!(
            "default `{literal:?}` does not fit the `{}` parameter type",
            kotlin_type(ty)
        ))),
    }
}
