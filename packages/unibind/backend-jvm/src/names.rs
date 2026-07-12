//! Java-side naming: the class, method, and exception names, plus the
//! `extern "C"` symbol names both sides agree on.

use heck::{ToLowerCamelCase as _, ToShoutySnakeCase as _, ToUpperCamelCase as _};
use proc_macro2::Ident;
use unibind_core::ir;

use crate::RenderError;

/// Java keywords and reserved literals that cannot name a method, argument,
/// or record component; a colliding name needs a `jvm(name = ...)` rename.
const JAVA_KEYWORDS: &[&str] = &[
    "abstract",
    "assert",
    "boolean",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extends",
    "false",
    "final",
    "finally",
    "float",
    "for",
    "goto",
    "if",
    "implements",
    "import",
    "instanceof",
    "int",
    "interface",
    "long",
    "native",
    "new",
    "null",
    "package",
    "private",
    "protected",
    "public",
    "return",
    "short",
    "static",
    "strictfp",
    "super",
    "switch",
    "synchronized",
    "this",
    "throw",
    "throws",
    "transient",
    "true",
    "try",
    "void",
    "volatile",
    "while",
];

/// Fail on a name Java cannot declare, saying where it came from.
pub fn checked(name: String, context: &str) -> Result<String, RenderError> {
    if JAVA_KEYWORDS.contains(&name.as_str()) {
        return Err(RenderError::new(format!(
            "{context} would be named `{name}`, a Java keyword; add a \
             `jvm(name = \"...\")` rename"
        )));
    }
    Ok(name)
}

/// The generated class: the `jvm(name = ...)` override, else the
/// `UpperCamelCase` of the Rust module name with the customary leading
/// underscore trimmed (`_scipql` -> `Scipql`).
pub fn class_name(interface: &ir::Interface) -> String {
    interface
        .names
        .jvm
        .clone()
        .unwrap_or_else(|| interface.name.trim_start_matches('_').to_upper_camel_case())
}

/// The key the generated class locates its library under: the
/// `unibind.library.<key>` system property (an explicit path), else
/// `System.mapLibraryName(<key>)` through the loader's search path.
pub fn library_key(interface: &ir::Interface) -> &str {
    interface.name.trim_start_matches('_')
}

/// The Java-facing name of a function: `lowerCamelCase` of the `jvm`
/// override or the Rust name.
pub fn method_name(function: &ir::Function) -> Result<String, RenderError> {
    let name = function
        .names
        .jvm
        .as_ref()
        .unwrap_or(&function.name)
        .to_lower_camel_case();
    checked(name, &format!("function `{}`", function.name))
}

/// Locals every generated method body declares; an argument landing on one
/// of these names would shadow them.
const RESERVED_LOCALS: &[&str] = &["args", "message", "reply", "result", "status", "variant"];

/// The Java-facing name of an argument.
pub fn arg_name(arg: &ir::Arg) -> Result<String, RenderError> {
    let name = arg
        .names
        .jvm
        .as_ref()
        .unwrap_or(&arg.name)
        .to_lower_camel_case();
    if RESERVED_LOCALS.contains(&name.as_str()) {
        return Err(RenderError::new(format!(
            "argument `{}` would be named `{name}`, which the generated \
             method bodies reserve; add a `jvm(name = \"...\")` rename",
            arg.name
        )));
    }
    checked(name, &format!("argument `{}`", arg.name))
}

/// The Java-facing name of a record (a nested `record` class).
pub fn record_name(record: &ir::Record) -> &str {
    record.names.jvm.as_deref().unwrap_or(&record.name)
}

/// The Java-facing name of the record declared with Rust name `name`.
pub fn record_name_of<'a>(interface: &'a ir::Interface, name: &'a str) -> &'a str {
    interface
        .records
        .iter()
        .find(|record| record.name == name)
        .map_or(name, record_name)
}

/// The Java-facing name of a record component.
pub fn component_name(record: &ir::Record, field: &ir::Field) -> Result<String, RenderError> {
    let name = field
        .names
        .jvm
        .as_ref()
        .unwrap_or(&field.name)
        .to_lower_camel_case();
    checked(
        name,
        &format!("field `{}` of record `{}`", field.name, record.name),
    )
}

/// The `jvm(name = ...)` override, else the Rust name with a trailing
/// `Error` swapped for the customary `Exception` (`ProbeError` ->
/// `ProbeException`).
fn exception_class(jvm_override: Option<&str>, rust_name: &str) -> String {
    jvm_override.map_or_else(
        || {
            format!(
                "{}Exception",
                rust_name.strip_suffix("Error").unwrap_or(rust_name)
            )
        },
        str::to_owned,
    )
}

/// The exception class carrying an error enum.
pub fn exception_name(error: &ir::ErrorType) -> String {
    exception_class(error.names.jvm.as_deref(), &error.name)
}

/// The exception class of the error declared with Rust name `name`.
pub fn exception_name_of(interface: &ir::Interface, name: &str) -> String {
    interface
        .errors
        .iter()
        .find(|error| error.name == name)
        .map_or_else(|| name.to_owned(), exception_name)
}

/// The nested exception class for one variant, inside its error's class.
pub fn variant_exception_name(variant: &ir::ErrorVariant) -> String {
    exception_class(variant.names.jvm.as_deref(), &variant.name)
}

/// The `SHOUTY_SNAKE` name of a function's `MethodHandle` constant; the
/// `H_` prefix keeps user names clear of the fixed plumbing constants.
pub fn handle_constant(function: &ir::Function) -> String {
    format!("H_{}", function.name.to_shouty_snake_case())
}

/// The `extern "C"` symbol carrying `function`, agreed on by both sides.
/// Rust names are unique per module, so the symbol is too.
pub fn symbol(interface: &ir::Interface, function: &ir::Function) -> String {
    format!("unibind_jvm_{}_{}", interface.name, function.name)
}

/// The `extern "C"` symbol reclaiming reply buffers for this interface.
pub fn free_symbol(interface: &ir::Interface) -> String {
    format!("unibind_jvm_{}_free", interface.name)
}

/// An identifier for a possibly-keyword Rust name (renames like `end` fall
/// back to raw identifiers).
pub fn name_ident(name: &str) -> Result<Ident, RenderError> {
    syn::parse_str::<Ident>(name)
        .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
        .map_err(|_| RenderError::new(format!("`{name}` is not usable as an identifier")))
}
