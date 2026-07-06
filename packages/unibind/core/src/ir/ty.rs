//! Boundary types and literal defaults.

use serde::{Deserialize, Serialize};

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
    /// A record or object declared in the same interface.
    Named(String),
    /// A `UniStream<T>`, an async iterator in the target language. Streams
    /// only appear in return position; the item type is owned.
    Stream(Box<Self>),
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
