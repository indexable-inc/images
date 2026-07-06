//! The unibind interface representation.
//!
//! One [`Interface`] value describes everything a `#[unibind::export]`
//! module exposes: functions, records, errors, and objects, with data enums
//! reserved for a later phase. Backends render it into binding code, and
//! [`crate::embed`] serializes it into the built artifact so out-of-process
//! generators can read the same contract.

mod data;
mod ty;

use serde::{Deserialize, Serialize};

pub use data::{Enum, EnumVariant, ErrorType, ErrorVariant, Field, Object, Record};
pub use ty::{FloatKind, IntKind, Literal, Type};

/// Version tag written into every serialized interface so a reader can
/// reject an IR layout it does not understand.
pub const IR_VERSION: u32 = 0;

/// Everything one `#[unibind::export]` module exposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interface {
    /// IR layout version, [`IR_VERSION`] when produced by this crate.
    pub version: u32,
    /// The Rust module identifier the interface was lowered from; the
    /// exported module name defaults to it.
    pub name: String,
    /// Per-language module renames.
    pub names: Names,
    /// Module doc comment, one entry per line.
    pub docs: Vec<String>,
    /// Exported free functions, in declaration order.
    pub functions: Vec<Function>,
    /// Plain-data structs, in declaration order.
    pub records: Vec<Record>,
    /// Plain data enums. Lowering still rejects them; the field keeps the
    /// serialized layout ready for when they land.
    pub enums: Vec<Enum>,
    /// Error enums, each rendered as an exception hierarchy in Python.
    pub errors: Vec<ErrorType>,
    /// Stateful handles, in declaration order.
    pub objects: Vec<Object>,
}

/// Per-language name overrides, from `#[unibind(py(name = "..."))]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Names {
    /// Python name override.
    pub py: Option<String>,
}

/// How a function suspends.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Asyncness {
    /// A plain blocking call.
    Sync,
    /// An `async fn`; backends surface it through the language's native
    /// event loop (a coroutine in Python).
    Async,
}

/// One exported free function; also the shape of object methods and
/// constructors, whose receiver is implied by their position in
/// [`Object`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Function {
    /// Rust function name.
    pub name: String,
    /// Per-language renames.
    pub names: Names,
    /// Doc comment lines; backends turn them into docstrings.
    pub docs: Vec<String>,
    /// Whether the function suspends.
    pub asyncness: Asyncness,
    /// Whether a sync body releases the GIL while it runs
    /// (`#[unibind(blocking)]`); never set together with
    /// [`Asyncness::Async`].
    #[serde(default)]
    pub blocking: bool,
    /// Arguments in declaration order.
    pub args: Vec<Arg>,
    /// Success type; `None` is unit.
    pub ret: Option<Type>,
    /// The `#[unibind::error]` enum named in the function's `Result`, if any.
    pub throws: Option<String>,
}

/// One function argument.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Arg {
    /// Rust argument name.
    pub name: String,
    /// Per-language renames.
    pub names: Names,
    /// Boundary type.
    pub ty: Type,
    /// Default value from `#[unibind(default = ...)]`. `Option` arguments
    /// without one default to `None` in rendered bindings.
    pub default: Option<Literal>,
}
