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

/// A closed set of named alternatives, none of which carries data.
///
/// The `#[unibind::enumeration]` shape. Each backend renders its own idiom
/// for it (a union of string literals in TypeScript, a `StrEnum` in
/// Python), and the value that actually crosses is the variant's
/// [`EnumVariant::wire`] string, identical in every language.
///
/// Variants with fields are a different intent -- a sum type -- and
/// lowering still rejects them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Enum {
    /// Rust enum name.
    pub name: String,
    /// Per-language renames of the *type*; the wire values are not
    /// renameable per language, because one string crosses to all of them.
    pub names: Names,
    /// Doc comment lines.
    pub docs: Vec<String>,
    /// Variants in declaration order.
    pub variants: Vec<EnumVariant>,
}

/// One unit variant of an [`Enum`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumVariant {
    /// Rust variant name.
    pub name: String,
    /// The string that crosses the boundary, decided at lowering from the
    /// Rust name and the enum's `rename_all` (serde's conventions, so the
    /// binding agrees with the JSON the same enum already serializes).
    /// Language-independent by construction: TypeScript compares this
    /// literal, Python holds it as a `StrEnum` value.
    pub wire: String,
    /// Per-language renames of the *member identifier*, for the languages
    /// that have one. Python's `StrEnum` member is named here (defaulting
    /// to `SCREAMING_SNAKE_CASE` of the Rust name); a TypeScript union has
    /// no identifier to rename, so the ts backend ignores it.
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
    /// Java base exception from `jvm(base = "...")`; `None` means
    /// `RuntimeException`. Additive to the serialized layout: absent in
    /// older IR payloads (and skipped when unset), so readers on either
    /// side of the change agree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jvm_base: Option<String>,
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
    /// Functions on the type rather than on an instance, in declaration
    /// order. Receiver-less like the constructor, but each keeps its own
    /// name and may be async, so one object can offer several. `ret`
    /// carries the real return type rather than implying the object, which
    /// is what lets one of these construct (`Machine.oci`) and another
    /// answer something else about the type (`Machine.list`).
    #[serde(default)]
    pub associated: Vec<Function>,
    /// Methods in declaration order; each implicitly takes `&self`.
    #[serde(default)]
    pub methods: Vec<Function>,
}
