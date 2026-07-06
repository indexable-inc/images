//! Overloads dropping trailing defaulted arguments, mirroring the Python
//! surface where trailing parameters carry defaults.

use unibind_core::ir;

use crate::ctype::CTy;
use crate::java::methods::ret_type;
use crate::java::{line, types};
use crate::{names, RenderError};

/// Overloads dropping trailing arguments that carry a default (an explicit
/// literal, or `null` for an `Option` without one).
pub fn overloads(function: &ir::Function) -> Result<Vec<String>, RenderError> {
    let mut defaults = Vec::new();
    for arg in &function.args {
        let cty = CTy::of(&arg.ty);
        let rendered = match &arg.default {
            Some(literal) => Some(types::java_literal(&cty, literal)?),
            None if matches!(cty, CTy::Option(_)) => Some("null".to_owned()),
            None => None,
        };
        defaults.push(rendered);
    }
    let total = function.args.len();
    let mut tail = 0;
    while tail < total && defaults[total - 1 - tail].is_some() {
        tail += 1;
    }

    let name = names::camel(&function.name);
    let link_types: Vec<String> = function
        .args
        .iter()
        .map(|arg| erased(&types::java_type(&CTy::of(&arg.ty), false)))
        .collect();
    let mut out = Vec::new();
    for dropped in 1..=tail {
        let kept = &function.args[..total - dropped];
        let mut call_args: Vec<String> = kept
            .iter()
            .map(|arg| names::camel(&arg.name))
            .collect();
        for default in &defaults[total - dropped..] {
            call_args.push(default.clone().expect("trailing run carries defaults"));
        }
        let mut text = String::new();
        line(
            &mut text,
            1,
            &format!(
                "/** Calls {{@link #{name}({})}} with default trailing arguments. */",
                link_types.join(", ")
            ),
        );
        let kept_params: Vec<String> = kept
            .iter()
            .map(|arg| {
                format!(
                    "{} {}",
                    types::java_type(&CTy::of(&arg.ty), false),
                    names::camel(&arg.name)
                )
            })
            .collect();
        line(
            &mut text,
            1,
            &format!(
                "public static {} {name}({}) {{",
                ret_type(function.ret.as_ref().map(CTy::of).as_ref()),
                kept_params.join(", ")
            ),
        );
        let call = format!("{name}({});", call_args.join(", "));
        if function.ret.is_some() {
            line(&mut text, 2, &format!("return {call}"));
        } else {
            line(&mut text, 2, &call);
        }
        line(&mut text, 1, "}");
        out.push(text);
    }
    Ok(out)
}

/// Generic-erased type text for a `{@link}` method signature.
fn erased(java: &str) -> String {
    java.split('<').next().unwrap_or(java).to_owned()
}

