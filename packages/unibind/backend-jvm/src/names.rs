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
    free_suffix(&export_symbol(module, function))
}

/// The `extern "C"` export for one object constructor or method; the
/// object keeps its Rust PascalCase inside the symbol.
pub fn object_export_symbol(module: &str, object: &str, method: &str) -> String {
    format!("unibind_jvm_{module}_{object}_{method}")
}

/// The export releasing one object handle (dropping its `Arc`).
pub fn object_free_symbol(module: &str, object: &str) -> String {
    format!("unibind_jvm_{module}_{object}__free")
}

/// The `__free` companion on an arbitrary export symbol; methods share the
/// suffix scheme with free functions, so companions build on the base
/// symbol rather than on module + function.
pub fn free_suffix(base: &str) -> String {
    format!("{base}__free")
}

/// The companion export requesting cancellation of an async export's task.
pub fn cancel_suffix(base: &str) -> String {
    format!("{base}__cancel")
}

/// The companion export releasing an async export's task handle.
pub fn task_free_suffix(base: &str) -> String {
    format!("{base}__task_free")
}

/// The companion export pulling one item from a stream export's handle.
pub fn stream_next_suffix(base: &str) -> String {
    format!("{base}__stream_next")
}

/// The companion export releasing a stream export's handle.
pub fn stream_free_suffix(base: &str) -> String {
    format!("{base}__stream_free")
}

/// The companion export freeing one of a stream export's item envelopes.
pub fn item_free_suffix(base: &str) -> String {
    format!("{base}__item_free")
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
