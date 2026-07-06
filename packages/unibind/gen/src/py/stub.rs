//! Render the `.pyi` stub for one interface.
//!
//! Ordering is deterministic: module docstring, imports (`collections.abc`
//! when a stream return needs it, `os` when an argument accepts a path),
//! errors, records, functions, and the trailing
//! `__version__` the generated `pymodule` sets. Everything keeps the
//! interface's declaration order within its group, so the stub diffs the way
//! the Rust module does.

use std::fmt::Write as _;

use unibind_core::ir;

use crate::py::types::{self, Position};

/// The complete `.pyi` text.
pub fn render(interface: &ir::Interface) -> String {
    let mut blocks = Vec::new();
    if !interface.docs.is_empty() {
        blocks.push(docstring(&interface.docs, 0));
    }
    let imports = imports(interface);
    if !imports.is_empty() {
        blocks.push(imports.join("\n"));
    }
    for error in &interface.errors {
        error_classes(error, &mut blocks);
    }
    for record in &interface.records {
        blocks.push(record_class(interface, record));
    }
    for function in &interface.functions {
        blocks.push(function_def(interface, function));
    }
    blocks.push("__version__: str".to_owned());
    join_blocks(&blocks)
}

/// Join top-level blocks with two blank lines and end with one newline.
pub fn join_blocks(blocks: &[String]) -> String {
    let mut out = blocks.join("\n\n\n");
    out.push('\n');
    out
}

/// A docstring at `indent` levels of four spaces. A single line closes on the
/// same line; multi-line content closes on its own line, blank doc lines stay
/// truly empty (no trailing indent).
pub fn docstring(lines: &[String], indent: usize) -> String {
    let pad = "    ".repeat(indent);
    if let [line] = lines {
        return format!("{pad}\"\"\"{line}\"\"\"");
    }
    let mut out = format!("{pad}\"\"\"{}\n", lines[0]);
    for line in &lines[1..] {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "{pad}{line}").expect("write to string");
        }
    }
    write!(out, "{pad}\"\"\"").expect("write to string");
    out
}

/// The stub's imports, alphabetized: `collections.abc` exactly when a
/// function returns a stream (its annotation is
/// `collections.abc.AsyncIterator`), `os` exactly when a path can appear in
/// argument position.
fn imports(interface: &ir::Interface) -> Vec<String> {
    let mut imports = Vec::new();
    if needs_abc_import(interface) {
        imports.push("import collections.abc".to_owned());
    }
    if needs_os_import(interface) {
        imports.push("import os".to_owned());
    }
    imports
}

fn needs_abc_import(interface: &ir::Interface) -> bool {
    interface
        .functions
        .iter()
        .any(|function| matches!(function.ret, Some(ir::Type::Stream(_))))
}

/// A path in argument position: function arguments and record constructor
/// arguments (fields).
fn needs_os_import(interface: &ir::Interface) -> bool {
    let function_args = interface
        .functions
        .iter()
        .flat_map(|function| function.args.iter())
        .map(|arg| &arg.ty);
    let constructor_args = interface
        .records
        .iter()
        .flat_map(|record| record.fields.iter())
        .map(|field| &field.ty);
    function_args.chain(constructor_args).any(types::mentions_path)
}

/// One class per error: the base (extending `py_base`, `Exception` when
/// unset), then one subclass per variant.
fn error_classes(error: &ir::ErrorType, blocks: &mut Vec<String>) {
    let base = types::py_name(&error.names, &error.name);
    let builtin = error.py_base.as_deref().unwrap_or("Exception");
    blocks.push(class_block(&format!("class {base}({builtin}):"), &error.docs));
    for variant in &error.variants {
        let class = types::py_name(&variant.names, &variant.name);
        blocks.push(class_block(&format!("class {class}({base}):"), &variant.docs));
    }
}

/// A class whose body is its docstring, or `...` without one.
fn class_block(header: &str, docs: &[String]) -> String {
    if docs.is_empty() {
        return format!("{header} ...");
    }
    format!("{header}\n{}", docstring(docs, 1))
}

/// A record: docstring, the positional `__init__` the generated `#[new]`
/// exposes, then one read-only property per field.
fn record_class(interface: &ir::Interface, record: &ir::Record) -> String {
    let name = types::py_name(&record.names, &record.name);
    let mut members = Vec::new();

    let params: Vec<String> = record
        .fields
        .iter()
        .map(|field| {
            let py = types::py_name(&field.names, &field.name);
            let ty = types::annotation(interface, &field.ty, Position::Argument);
            format!("{py}: {ty}")
        })
        .collect();
    let mut init_params = String::from("self");
    for param in &params {
        init_params.push_str(", ");
        init_params.push_str(param);
    }
    members.push(format!("    def __init__({init_params}) -> None: ..."));

    for field in &record.fields {
        let py = types::py_name(&field.names, &field.name);
        let ty = types::annotation(interface, &field.ty, Position::Return);
        let header = format!("    @property\n    def {py}(self) -> {ty}:");
        if field.docs.is_empty() {
            members.push(format!("{header} ..."));
        } else {
            members.push(format!("{header}\n{}", docstring(&field.docs, 2)));
        }
    }

    let mut out = format!("class {name}:\n");
    if !record.docs.is_empty() {
        out.push_str(&docstring(&record.docs, 1));
        out.push_str("\n\n");
    }
    out.push_str(&members.join("\n\n"));
    out
}

/// A function stub: literal defaults, `None` for undefaulted `Option`
/// arguments, and a docstring that names the raised exception base when the
/// function throws (stub signatures cannot express `raises`). Async functions
/// render `async def`: the extension returns a coroutine resolving to the
/// annotated type.
fn function_def(interface: &ir::Interface, function: &ir::Function) -> String {
    let name = types::py_name(&function.names, &function.name);
    let params: Vec<String> = function.args.iter().map(|arg| parameter(interface, arg)).collect();
    let ret = function.ret.as_ref().map_or_else(
        || "None".to_owned(),
        |ty| types::annotation(interface, ty, Position::Return),
    );
    let def = match function.asyncness {
        ir::Asyncness::Async => "async def",
        ir::Asyncness::Sync => "def",
    };
    let header = format!("{def} {name}({}) -> {ret}:", params.join(", "));

    let mut doc_lines = function.docs.clone();
    if let Some(throws) = &function.throws {
        let base = error_base_py_name(interface, throws);
        if !doc_lines.is_empty() {
            doc_lines.push(String::new());
        }
        doc_lines.push(format!("Raises {base}."));
    }
    if doc_lines.is_empty() {
        return format!("{header} ...");
    }
    format!("{header}\n{}", docstring(&doc_lines, 1))
}

fn parameter(interface: &ir::Interface, arg: &ir::Arg) -> String {
    let py = types::py_name(&arg.names, &arg.name);
    let ty = types::annotation(interface, &arg.ty, Position::Argument);
    if let Some(default) = &arg.default {
        return format!("{py}: {ty} = {}", types::literal(default));
    }
    // `Option` arguments without an explicit default get `None` in the
    // generated `#[pyo3(signature = ...)]`; mirror that here.
    if matches!(arg.ty, ir::Type::Option(_)) {
        return format!("{py}: {ty} = None");
    }
    format!("{py}: {ty}")
}

/// The Python name of the exception base class a `throws` names.
fn error_base_py_name(interface: &ir::Interface, throws: &str) -> String {
    interface
        .errors
        .iter()
        .find(|error| error.name == throws)
        .map_or(throws, |error| types::py_name(&error.names, &error.name))
        .to_owned()
}
