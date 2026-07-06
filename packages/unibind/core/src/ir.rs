//! The unibind interface representation.
//!
//! One [`Interface`] value describes everything a `#[unibind::export]`
//! module exposes: functions, records, and errors today, with enums and
//! objects reserved for later phases. Backends render it into binding code,
//! and [`crate::embed`] serializes it into the built artifact so
//! out-of-process generators can read the same contract.

use serde::{Deserialize, Serialize};

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
    /// Plain data enums. Phase 0 lowering rejects them; the field keeps the
    /// serialized layout ready for when they land.
    pub enums: Vec<Enum>,
    /// Error enums, each rendered as an exception hierarchy in Python.
    pub errors: Vec<ErrorType>,
    /// Stateful handles. Phase 0 lowering rejects them (they land with
    /// resources in phase 2, issue #1992); the field keeps the layout ready.
    pub objects: Vec<Object>,
}

/// Per-language name overrides, from `#[unibind(py(name = "..."))]`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Names {
    /// Python name override.
    pub py: Option<String>,
}

/// How a function suspends. Phase 0 lowers only `Sync`; `Async` is the
/// phase 2 surface (issue #1992).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Asyncness {
    /// A plain blocking call.
    Sync,
    /// An `async fn`; reserved, never produced by phase 0 lowering.
    Async,
}

/// One exported free function.
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

/// A literal default from `#[unibind(default = ...)]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Literal {
    /// `true` / `false`.
    Bool(bool),
    /// An integer literal (optionally negated).
    Int(i64),
    /// A float literal (optionally negated).
    Float(f64),
    /// A string literal.
    Str(String),
    /// The `None` default for an `Option` argument.
    None,
}

/// A type at the binding boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Type {
    /// `bool`.
    Bool,
    /// A fixed-width integer.
    Int(IntKind),
    /// A float.
    Float(FloatKind),
    /// UTF-8 text: `String` when `owned`, `&str` otherwise.
    String {
        /// Whether the Rust side takes ownership.
        owned: bool,
    },
    /// A filesystem path: `PathBuf` when `owned`, `&Path` otherwise.
    Path {
        /// Whether the Rust side takes ownership.
        owned: bool,
    },
    /// Binary data: `Vec<u8>` when `owned`, `&[u8]` otherwise.
    Bytes {
        /// Whether the Rust side takes ownership.
        owned: bool,
    },
    /// `Option<T>`.
    Option(Box<Self>),
    /// `Vec<T>` (except `Vec<u8>`, which lowers to [`Type::Bytes`]).
    Vec(Box<Self>),
    /// `HashMap<K, V>`.
    Map {
        /// Key type; phase 0 restricts it to strings and integers.
        key: Box<Self>,
        /// Value type.
        value: Box<Self>,
    },
    /// A record declared in the same interface.
    Named(String),
}

/// Integer width and signedness.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum IntKind {
    I8,
    I16,
    I32,
    I64,
    Isize,
    U8,
    U16,
    U32,
    U64,
    Usize,
}

/// Float width.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FloatKind {
    F32,
    F64,
}

/// A plain-data struct crossing the boundary by value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// Rust struct name.
    pub name: String,
    /// Per-language renames.
    pub names: Names,
    /// Doc comment lines.
    pub docs: Vec<String>,
    /// Fields in declaration order.
    pub fields: Vec<Field>,
}

/// One record field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    /// Rust field name.
    pub name: String,
    /// Per-language renames.
    pub names: Names,
    /// Doc comment lines.
    pub docs: Vec<String>,
    /// Field type; always owned.
    pub ty: Type,
}

/// A plain data enum. Reserved: phase 0 lowering rejects enums.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Enum {
    /// Rust enum name.
    pub name: String,
    /// Per-language renames.
    pub names: Names,
    /// Doc comment lines.
    pub docs: Vec<String>,
    /// Variants in declaration order.
    pub variants: Vec<EnumVariant>,
}

/// One data enum variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumVariant {
    /// Rust variant name.
    pub name: String,
    /// Per-language renames.
    pub names: Names,
    /// Doc comment lines.
    pub docs: Vec<String>,
}

/// An error enum, rendered as an exception hierarchy: one base class for the
/// enum and one subclass per variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorType {
    /// Rust enum name; also the base exception class name.
    pub name: String,
    /// Per-language renames for the base class.
    pub names: Names,
    /// Doc comment lines.
    pub docs: Vec<String>,
    /// Python base exception from `py(base = "...")`; `None` means
    /// `Exception`.
    pub py_base: Option<String>,
    /// Variants in declaration order, each an exception subclass.
    pub variants: Vec<ErrorVariant>,
}

/// One error variant. Its fields stay on the Rust side; the rendered
/// exception carries the variant's `Display` text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorVariant {
    /// Rust variant name; also the exception subclass name.
    pub name: String,
    /// Per-language renames.
    pub names: Names,
    /// Doc comment lines.
    pub docs: Vec<String>,
}

/// A stateful handle. Reserved: phase 0 lowering rejects
/// `#[unibind::object]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object {
    /// Rust type name.
    pub name: String,
    /// Per-language renames.
    pub names: Names,
    /// Doc comment lines.
    pub docs: Vec<String>,
}
