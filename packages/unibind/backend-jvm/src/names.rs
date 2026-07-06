//! Naming: `snake_case` Rust names to Java and Kotlin spellings, plus the
//! exported symbol names the Rust glue and the Java binding share.

use crate::RenderError;

/// `snake_case` to `camelCase`.
pub fn camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper_next = false;
    for character in name.chars() {
        if character == '_' {
            upper_next = true;
        } else if upper_next {
            out.extend(character.to_uppercase());
            upper_next = false;
        } else {
            out.push(character);
        }
    }
    out
}

/// `snake_case` to `PascalCase`.
pub fn pascal(name: &str) -> String {
    let camel = camel(name.trim_start_matches('_'));
    let mut characters = camel.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

/// A type name with its first character lowered, for generated Java helper
/// methods (`SampleError` becomes `sampleErrorException`).
pub fn decapitalize(name: &str) -> String {
    let mut characters = name.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(characters).collect()
    })
}

/// The `extern "C"` export for one function.
pub fn export_symbol(module: &str, function: &str) -> String {
    format!("unibind_jvm_{module}_{function}")
}

/// The companion export that frees one function's return envelope.
pub fn free_symbol(module: &str, function: &str) -> String {
    format!("{}__free", export_symbol(module, function))
}

/// The ABI version probe the Java binding calls at load.
pub fn abi_symbol(module: &str) -> String {
    format!("unibind_jvm_{module}_abi_version")
}

/// The Java package one interface lands in.
pub fn java_package(module: &str) -> String {
    format!("unibind.{module}")
}

/// The `SCREAMING_SNAKE` method-handle constant for one function.
pub fn handle_const(function: &str) -> String {
    format!("H_{}", function.to_uppercase())
}

/// An identifier for a Rust-side name coming out of the IR.
pub fn rust_ident(name: &str) -> Result<syn::Ident, RenderError> {
    syn::parse_str::<syn::Ident>(name)
        .or_else(|_| syn::parse_str::<syn::Ident>(&format!("r#{name}")))
        .map_err(|_| RenderError::new(format!("`{name}` is not usable as an identifier")))
}
