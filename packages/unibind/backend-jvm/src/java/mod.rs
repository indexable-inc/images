//! Generate the Java 22 Panama (`java.lang.foreign`) binding sources.

mod decode;
mod encode;
mod errors;
mod methods;
mod module_class;
mod overloads;
mod records;
pub mod types;

use std::collections::BTreeMap;

use unibind_core::ir;

use crate::ctype::CTy;
use crate::model::Model;
use crate::{names, RenderError, SourceFile};

/// Generate the Java sources for one interface: the module class, one file
/// per record, one exception hierarchy per error enum, and the shared panic
/// exception.
///
/// # Errors
///
/// Fails for surface the sync JVM backend does not implement (async
/// functions, data enums, objects) and for unresolved or recursive record
/// types.
pub fn generate_java(interface: &ir::Interface) -> Result<Vec<SourceFile>, RenderError> {
    let model = Model::new(interface)?;
    let dir = format!("unibind/{}", interface.name);
    let mut files = vec![SourceFile {
        path: format!("{dir}/{}.java", names::pascal(&interface.name)),
        content: module_class::render(interface, &model)?,
    }];
    for record in &interface.records {
        files.push(SourceFile {
            path: format!("{dir}/{}.java", record.name),
            content: records::render(interface, record),
        });
    }
    for error in &interface.errors {
        files.push(SourceFile {
            path: format!("{dir}/{}Exception.java", error.name),
            content: errors::render(interface, error),
        });
    }
    files.push(SourceFile {
        path: format!("{dir}/UnibindPanicException.java"),
        content: errors::panic_exception(interface),
    });
    Ok(files)
}

/// Push one line at `indent` levels of four spaces; empty text is a blank
/// line without trailing spaces.
pub fn line(out: &mut String, indent: usize, text: &str) {
    if text.is_empty() {
        out.push('\n');
        return;
    }
    for _ in 0..indent {
        out.push_str("    ");
    }
    out.push_str(text);
    out.push('\n');
}

/// The aggregates the module class needs an encode helper for: everything
/// reachable from argument types.
pub fn encode_set(interface: &ir::Interface, model: &Model<'_>) -> BTreeMap<String, CTy> {
    model.reachable_aggregates(
        interface
            .functions
            .iter()
            .flat_map(|function| function.args.iter().map(|arg| &arg.ty)),
    )
}

/// The aggregates the module class needs a decode helper for: everything
/// reachable from return types, plus text for every envelope's `err_msg`.
pub fn decode_set(interface: &ir::Interface, model: &Model<'_>) -> BTreeMap<String, CTy> {
    let mut set = model.reachable_aggregates(
        interface
            .functions
            .iter()
            .filter_map(|function| function.ret.as_ref()),
    );
    set.entry(CTy::Str.mangle()).or_insert(CTy::Str);
    set
}
