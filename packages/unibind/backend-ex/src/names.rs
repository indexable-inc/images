//! Elixir-side naming: module namespaces, function and argument names,
//! and identifiers for the generated Rust glue.

use heck::{ToSnakeCase as _, ToUpperCamelCase as _};
use proc_macro2::Ident;
use unibind_core::ir;

use crate::RenderError;

/// The Elixir namespace module: the `ex(name = ...)` override, else the
/// `UpperCamelCase` of the Rust module name with the customary leading
/// underscore trimmed (`_scipql` -> `Scipql`).
pub fn ns_name(interface: &ir::Interface) -> String {
    interface
        .names
        .ex
        .clone()
        .unwrap_or_else(|| interface.name.trim_start_matches('_').to_upper_camel_case())
}

/// The `snake_case` of the namespace, naming the OTP app and the `.ex` files.
pub fn ns_snake(interface: &ir::Interface) -> String {
    ns_name(interface).to_snake_case()
}

/// The Elixir-facing name of a function: `snake_case` of the `ex` override
/// or the Rust name. Also the name the NIF registers under.
pub fn ex_fn_name(function: &ir::Function) -> String {
    function
        .names
        .ex
        .as_ref()
        .unwrap_or(&function.name)
        .to_snake_case()
}

/// The Elixir-facing name of an argument.
pub fn ex_arg_name(arg: &ir::Arg) -> String {
    arg.names.ex.as_ref().unwrap_or(&arg.name).to_snake_case()
}

/// The Elixir-facing name of a record (a module name, so CamelCase).
pub fn ex_record_name(record: &ir::Record) -> &str {
    record.names.ex.as_deref().unwrap_or(&record.name)
}

/// The Elixir-facing name of an error type.
pub fn ex_error_name(error: &ir::ErrorType) -> &str {
    error.names.ex.as_deref().unwrap_or(&error.name)
}

/// The Elixir-facing name of an object.
pub fn ex_object_name(object: &ir::Object) -> &str {
    object.names.ex.as_deref().unwrap_or(&object.name)
}

/// The Elixir-facing name of the record declared with Rust name `name`.
pub fn ex_record_name_of<'a>(interface: &'a ir::Interface, name: &'a str) -> &'a str {
    interface
        .records
        .iter()
        .find(|record| record.name == name)
        .map_or(name, ex_record_name)
}

/// The Elixir-facing name of the error declared with Rust name `name`.
pub fn ex_error_name_of<'a>(interface: &'a ir::Interface, name: &'a str) -> &'a str {
    interface
        .errors
        .iter()
        .find(|error| error.name == name)
        .map_or(name, ex_error_name)
}

/// The atom naming an error variant: `snake_case` of the `ex` override or
/// the Rust variant name.
pub fn variant_atom(variant: &ir::ErrorVariant) -> String {
    variant
        .names
        .ex
        .as_ref()
        .unwrap_or(&variant.name)
        .to_snake_case()
}

/// The registered NIF name of an object member: `<object>_<function>`, so
/// two objects can both have `new`.
pub fn member_nif_name(object: &ir::Object, function: &ir::Function) -> String {
    format!(
        "{}_{}",
        ex_object_name(object).to_snake_case(),
        ex_fn_name(function)
    )
}

/// An identifier for a possibly-keyword name (renames like `end` fall back
/// to raw identifiers).
pub fn name_ident(name: &str) -> Result<Ident, RenderError> {
    syn::parse_str::<Ident>(name)
        .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
        .map_err(|_| RenderError::new(format!("`{name}` is not usable as an identifier")))
}
