//! Declared data shapes: records, enums, errors, and objects.

use serde::{Deserialize, Serialize};

use super::{Function, Names, Type};

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

/// A plain data enum. Reserved: lowering rejects enums until they land.
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

/// A stateful handle the target language holds by reference: the backend
/// wraps the struct rather than copying it field by field, so its fields
/// never cross the boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Object {
    /// Rust type name.
    pub name: String,
    /// Per-language renames.
    pub names: Names,
    /// Doc comment lines.
    pub docs: Vec<String>,
    /// Whether the object is a resource: it declares a `close` method the
    /// bindings surface as `close()` / async-with, warning when it never
    /// runs.
    #[serde(default)]
    pub resource: bool,
    /// The receiver-less constructor, if any. Its `ret` is `None` (the
    /// object itself is implied); `throws` may name an error.
    #[serde(default)]
    pub constructor: Option<Function>,
    /// Methods in declaration order; each implicitly takes `&self`.
    #[serde(default)]
    pub methods: Vec<Function>,
}
