//! Render the wrapper `__init__.py`.
//!
//! The wrapper re-exports every public name from the native extension module
//! so `import <package>` presents the boundary directly. A package that
//! layers hand-written Python over the extension (the scipql shape) keeps
//! its own `__init__.py` and the caller passes `--skip-init` instead.

use std::fmt::Write as _;

use unibind_core::ir;

use crate::py::stub;
use crate::py::types;

/// The complete `__init__.py` text.
pub fn render(interface: &ir::Interface, module_name: &str) -> String {
    let mut names = public_names(interface);
    names.push("__version__".to_owned());
    names.sort();

    let mut out = String::new();
    if !interface.docs.is_empty() {
        out.push_str(&stub::docstring(&interface.docs, 0));
        out.push_str("\n\n");
    }
    writeln!(out, "from .{module_name} import (").expect("write to string");
    for name in &names {
        writeln!(out, "    {name},").expect("write to string");
    }
    out.push_str(")\n\n__all__ = [\n");
    for name in &names {
        writeln!(out, "    \"{name}\",").expect("write to string");
    }
    out.push_str("]\n");
    out
}

/// Every name the extension module registers: functions, record classes, and
/// the error base plus its variant subclasses (Python names throughout).
fn public_names(interface: &ir::Interface) -> Vec<String> {
    let mut names = Vec::new();
    for function in &interface.functions {
        names.push(types::py_name(&function.names, &function.name).to_owned());
    }
    for record in &interface.records {
        names.push(types::py_name(&record.names, &record.name).to_owned());
    }
    for error in &interface.errors {
        names.push(types::py_name(&error.names, &error.name).to_owned());
        for variant in &error.variants {
            names.push(types::py_name(&variant.names, &variant.name).to_owned());
        }
    }
    names
}
