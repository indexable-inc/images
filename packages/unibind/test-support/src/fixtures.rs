//! Literal-IR fixture builders shared by the emitter snapshot tests.
//!
//! Every backend's snapshot test builds the same shapes and differs only
//! in which rename slot its `names` wrapper fills, so the constructors
//! live here once and each test keeps only that wrapper.

use unibind_core::ir;

/// Docs lines as the IR stores them.
#[must_use]
pub fn docs(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|line| (*line).to_owned()).collect()
}

/// An argument with no rename in any language.
#[must_use]
pub fn arg(name: &str, ty: ir::Type, default: Option<ir::Literal>) -> ir::Arg {
    ir::Arg {
        name: name.to_owned(),
        names: ir::Names::default(),
        ty,
        default,
    }
}

/// A sync, non-throwing, returnless function; tests override the rest
/// through struct update syntax.
#[must_use]
pub fn function(
    name: &str,
    names: ir::Names,
    doc_lines: &[&str],
    args: Vec<ir::Arg>,
) -> ir::Function {
    ir::Function {
        name: name.to_owned(),
        names,
        docs: docs(doc_lines),
        asyncness: ir::Asyncness::Sync,
        blocking: false,
        args,
        ret: None,
        throws: None,
    }
}
