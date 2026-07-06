//! Render a lowered [`unibind_core::ir::Interface`] into JVM binding code.
//!
//! Three artifacts come out of the same interface, and all three consume the
//! one layout model in [`ctype`] / [`model`], so the two sides of the
//! boundary can never disagree about a struct layout or a symbol name:
//!
//! - [`render`]: a hidden Rust glue module of plain `extern "C"` exports
//!   (no JNI, no binding library), spliced into the user's crate by the
//!   `unibind` macro when its `jvm` feature is on.
//! - [`generate_java`]: Java 22 Panama (`java.lang.foreign`) sources that
//!   call those exports through `Linker#downcallHandle`, reading and writing
//!   mirror structs at the model's precomputed offsets.
//! - [`generate_kotlin`]: a thin Kotlin sugar layer delegating to the Java
//!   binding (default parameter values, nullable types); never a second FFI
//!   path.

pub mod ctype;
mod java;
mod kotlin;
pub mod model;
mod names;
mod rust_glue;

pub use java::generate_java;
pub use kotlin::generate_kotlin;
pub use rust_glue::render;

/// The rendered Rust glue for one interface.
#[derive(Debug)]
pub struct RenderedJvm {
    /// Sibling items for the exported module: one hidden module holding the
    /// C mirror types, their layout assertions, and the `extern "C"`
    /// exports.
    pub glue: proc_macro2::TokenStream,
}

/// One generated host-language source file.
#[derive(Debug)]
pub struct SourceFile {
    /// Slash-separated path relative to the source root, following the Java
    /// package, e.g. `unibind/sample/Row.java`.
    pub path: String,
    /// Complete file text.
    pub content: String,
}

/// A rendering failure; the macro positions it at the exported module.
#[derive(Debug)]
pub struct RenderError {
    /// What went wrong and what to do instead.
    pub message: String,
}

impl RenderError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
