//! The JavaScript spelling of every exported name.
//!
//! `wasm-bindgen` leaves a Rust name exactly as written unless the item
//! carries `js_name`, and `snake_case` is not the vocabulary a JavaScript
//! caller expects. So the glue names every export explicitly: the interface's
//! `ts(name = "...")` rename when it declares one -- the wasm surface *is*
//! JavaScript, so the ts renames are its renames -- and napi's own
//! `camelCase` conversion otherwise, which keeps one JavaScript vocabulary
//! across the two backends that both target it.

use unibind_core::casing::lower_camel_case;
use unibind_core::ir;

/// The JavaScript name of a function, method, or record field.
pub fn js_member(names: &ir::Names, rust: &str) -> String {
    names
        .ts
        .clone()
        .unwrap_or_else(|| lower_camel_case(rust))
}

/// The JavaScript name of a generated class. Rust type names are already
/// `PascalCase`, which is also JavaScript's convention for a class, so an
/// unrenamed type keeps its own name.
pub fn js_type(names: &ir::Names, rust: &str) -> String {
    names.ts.clone().unwrap_or_else(|| rust.to_owned())
}
