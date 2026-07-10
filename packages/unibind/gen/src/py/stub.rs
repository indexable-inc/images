//! Render the `.pyi` stub for one interface.
//!
//! Ordering is deterministic: module docstring, imports (`os` when an
//! argument accepts a path), errors, records, object classes, stream
//! classes, functions, and the trailing `__version__` the generated
//! `pymodule` sets. Everything keeps the interface's declaration order
//! within its group, so the stub diffs the way the Rust module does.

use std::fmt::Write as _;

use unibind_core::ir;

use crate::py::streams::{self, StreamExport};
use crate::py::types::{self, Position};

/// The complete `.pyi` text.
pub fn render(interface: &ir::Interface) -> String {
    let mut blocks = Vec::new();
    if !interface.docs.is_empty() {
        blocks.push(docstring(&interface.docs, 0));
    }
    if needs_os_import(interface) {
        blocks.push("import os".to_owned());
    }
    for error in &interface.errors {
        error_classes(error, &mut blocks);
    }
    for record in &interface.records {
        blocks.push(record_class(interface, record));
    }
    for object in &interface.objects {
        blocks.push(object_class(interface, object));
    }
    // Forward references need no quoting in a stub, so stream classes can
    // trail the objects whose methods return them.
    for export in streams::collect(interface) {
        blocks.push(stream_class(interface, &export));
    }
    for function in &interface.functions {
        blocks.push(callable_def(interface, function, &Receiver::Free));
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

/// A path in argument position needs `import os` for the `os.PathLike`
/// form: function, method, and constructor arguments, plus record
/// constructor arguments (fields).
fn needs_os_import(interface: &ir::Interface) -> bool {
    let function_args = interface
        .functions
        .iter()
        .flat_map(|function| function.args.iter())
        .map(|arg| &arg.ty);
    let record_args = interface
        .records
        .iter()
        .flat_map(|record| record.fields.iter())
        .map(|field| &field.ty);
    let object_args = interface
        .objects
        .iter()
        .flat_map(|object| object.constructor.iter().chain(object.methods.iter()))
        .flat_map(|function| function.args.iter())
        .map(|arg| &arg.ty);
    function_args
        .chain(record_args)
        .chain(object_args)
        .any(types::mentions_path)
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

/// A class whose body is a docstring plus member blocks separated by one
/// blank line.
fn class_with_members(header: &str, docs: &[String], members: &[String]) -> String {
    if members.is_empty() {
        return class_block(header, docs);
    }
    let mut out = format!("{header}\n");
    if !docs.is_empty() {
        out.push_str(&docstring(docs, 1));
        out.push_str("\n\n");
    }
    out.push_str(&members.join("\n\n"));
    out
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

    class_with_members(&format!("class {name}:"), &record.docs, &members)
}

/// An object: docstring, the `__init__` its declared constructor exposes
/// through `#[new]` (no constructor means the class cannot be instantiated
/// from Python, so no `__init__` appears), its methods, and, for
/// resources, the close/async-with surface the pyo3 backend generates.
fn object_class(interface: &ir::Interface, object: &ir::Object) -> String {
    let name = types::py_name(&object.names, &object.name);
    let mut members = Vec::new();
    if let Some(ctor) = &object.constructor {
        members.push(constructor_def(interface, ctor));
    }
    let close = resource_close(object);
    for method in &object.methods {
        // The resource surface owns `close`; the backend skips the generic
        // rendering for it, so the stub does too.
        if close.is_some_and(|close| std::ptr::eq(close, method)) {
            continue;
        }
        let receiver = Receiver::Method {
            object: &object.name,
        };
        members.push(callable_def(interface, method, &receiver));
    }
    if let Some(close) = close {
        members.push(close_def(interface, close));
        members.push(aenter_def(name));
        members.push(aexit_def());
    }
    class_with_members(&format!("class {name}:"), &object.docs, &members)
}

/// The user close method the resource surface wraps: named `close`, zero
/// arguments, no success value (the shape lowering guarantees resources
/// declare). `None` for plain objects.
fn resource_close(object: &ir::Object) -> Option<&ir::Function> {
    if !object.resource {
        return None;
    }
    object
        .methods
        .iter()
        .find(|method| method.name == "close" && method.args.is_empty() && method.ret.is_none())
}

/// The generated `close()`: idempotent in the runtime, async exactly when
/// the user's close is.
fn close_def(interface: &ir::Interface, close: &ir::Function) -> String {
    let def = def_keyword(close.asyncness);
    let header = format!("    {def} close(self) -> None:");
    def_block(&header, &doc_lines_with_raises(interface, close), 1)
}

/// `__aenter__` resolves to the object itself; its docstring mirrors the
/// generated method's.
fn aenter_def(class: &str) -> String {
    let docs = vec!["Enter `async with`: resolves to the object itself.".to_owned()];
    def_block(&format!("    async def __aenter__(self) -> {class}:"), &docs, 1)
}

/// `__aexit__` closes and resolves to `False`; the generated method takes
/// the exception triple as raw objects.
fn aexit_def() -> String {
    let docs =
        vec!["Exit `async with`: closes the resource, never suppresses the exception.".to_owned()];
    def_block(
        "    async def __aexit__(self, exc_type: object, exc: object, tb: object) -> bool:",
        &docs,
        1,
    )
}

/// The constructor stub: `#[new]` surfaces as `__init__`, with the same
/// parameter surface as any callable.
fn constructor_def(interface: &ir::Interface, ctor: &ir::Function) -> String {
    let mut params = String::from("self");
    for arg in &ctor.args {
        params.push_str(", ");
        params.push_str(&parameter(interface, arg));
    }
    let header = format!("    def __init__({params}) -> None:");
    def_block(&header, &doc_lines_with_raises(interface, ctor), 1)
}

/// A per-export stream class, mirroring the pyo3 backend's generated
/// async-iterator classes (`__aiter__` returns the class itself,
/// `__anext__` resolves one item) and their synthesized docstrings.
fn stream_class(interface: &ir::Interface, export: &StreamExport<'_>) -> String {
    let class = streams::class_name(export.owner, &export.function.name);
    let produced = export.owner.map_or_else(
        || export.function.name.clone(),
        |object| format!("{object}.{}", export.function.name),
    );
    let docs = vec![
        format!("Async iterator produced by `{produced}`."),
        String::new(),
        "Pull-based: each `__anext__` polls exactly one item, so the producer only runs as \
         fast as the consumer awaits."
            .to_owned(),
    ];
    let item = types::annotation(interface, export.item, Position::Return);
    let members = vec![
        format!("    def __aiter__(self) -> {class}: ..."),
        format!("    async def __anext__(self) -> {item}: ..."),
    ];
    class_with_members(&format!("class {class}:"), &docs, &members)
}

/// Whose callable a stub renders: a module-level function, or a method
/// (implicit `self`) of the named object. The receiver decides indentation
/// and which per-export stream class a stream return names.
enum Receiver<'a> {
    Free,
    Method { object: &'a str },
}

/// A function or method stub: literal defaults, `None` for undefaulted
/// `Option` arguments, and a docstring that names the raised exception base
/// when the callable throws (stub signatures cannot express `raises`).
/// Async callables render `async def`: the extension returns a coroutine
/// resolving to the annotated type.
fn callable_def(
    interface: &ir::Interface,
    function: &ir::Function,
    receiver: &Receiver<'_>,
) -> String {
    let (indent, owner) = match receiver {
        Receiver::Free => (0, None),
        Receiver::Method { object } => (1, Some(*object)),
    };
    let name = types::py_name(&function.names, &function.name);
    let mut params = Vec::new();
    if owner.is_some() {
        params.push("self".to_owned());
    }
    params.extend(function.args.iter().map(|arg| parameter(interface, arg)));
    let ret = match &function.ret {
        None => "None".to_owned(),
        // The runtime wraps a stream return in its per-export class; the
        // annotation names that class rather than the abstract iterator.
        Some(ir::Type::Stream(_)) => streams::class_name(owner, &function.name),
        Some(ty) => types::annotation(interface, ty, Position::Return),
    };
    let def = def_keyword(function.asyncness);
    let pad = "    ".repeat(indent);
    let header = format!("{pad}{def} {name}({}) -> {ret}:", params.join(", "));
    def_block(&header, &doc_lines_with_raises(interface, function), indent)
}

const fn def_keyword(asyncness: ir::Asyncness) -> &'static str {
    match asyncness {
        ir::Asyncness::Async => "async def",
        ir::Asyncness::Sync => "def",
    }
}

/// A def whose body is its docstring, or `...` without one.
fn def_block(header: &str, doc_lines: &[String], indent: usize) -> String {
    if doc_lines.is_empty() {
        return format!("{header} ...");
    }
    format!("{header}\n{}", docstring(doc_lines, indent + 1))
}

/// The callable's doc lines plus a trailing `Raises <base>.` naming the
/// exception base class when it throws.
fn doc_lines_with_raises(interface: &ir::Interface, function: &ir::Function) -> Vec<String> {
    let mut doc_lines = function.docs.clone();
    if let Some(throws) = &function.throws {
        let base = error_base_py_name(interface, throws);
        if !doc_lines.is_empty() {
            doc_lines.push(String::new());
        }
        doc_lines.push(format!("Raises {base}."));
    }
    doc_lines
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
