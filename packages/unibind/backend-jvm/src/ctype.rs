//! The C mirror model: mirror shapes and their `#[repr(C)]` layout math.
//!
//! [`CTy`] names the mirror of every boundary type; the layout helpers here
//! (used through [`crate::model::Model`]) are the single source of truth
//! both the Rust glue generator and the Java generator consume, so the two
//! sides of the boundary always agree on sizes and field offsets. The Rust
//! glue additionally emits `const` assertions comparing these numbers
//! against `core::mem::{size_of, align_of, offset_of}`, so any divergence
//! is a compile error rather than a runtime corruption.
//!
//! Layouts follow `#[repr(C)]` natural alignment on 64-bit targets:
//! pointers and `usize` are 8 bytes wide and no mirror exceeds alignment 8.

use unibind_core::ir;

/// Pointer + length pairs (`CString`, `CVec`) on a 64-bit target.
pub(crate) const PTR_PAIR: Layout = Layout { size: 16, align: 8 };

/// The C-level mirror of one boundary type.
#[derive(Debug, Clone)]
pub enum CTy {
    /// `bool` crossing as `u8` (0 or 1).
    Bool,
    /// A same-width integer.
    Int(ir::IntKind),
    /// A same-width float.
    Float(ir::FloatKind),
    /// UTF-8 text as `CString { ptr, len }`.
    Str,
    /// A filesystem path as UTF-8 text (`CPath`, the `CString` shape).
    Path,
    /// Raw bytes (`CBytes`, the `CString` shape).
    Bytes,
    /// `COption<T>`: a `u8` presence flag with the value inline, zeroed
    /// when absent.
    Option(Box<Self>),
    /// `CVec<T>`: a boxed slice as pointer + length.
    Vec(Box<Self>),
    /// A map as `CVec<CPair<K, V>>`.
    Map {
        /// Key mirror.
        key: Box<Self>,
        /// Value mirror.
        value: Box<Self>,
    },
    /// A record's generated `#[repr(C)]` mirror struct (`<Name>C`).
    Record(String),
}

impl CTy {
    /// The mirror of one IR type. Ownership never changes the mirror:
    /// borrowed and owned strings cross identically.
    #[must_use]
    pub fn of(ty: &ir::Type) -> Self {
        match ty {
            ir::Type::Bool => Self::Bool,
            ir::Type::Int(kind) => Self::Int(*kind),
            ir::Type::Float(kind) => Self::Float(*kind),
            ir::Type::String { .. } => Self::Str,
            ir::Type::Path { .. } => Self::Path,
            ir::Type::Bytes { .. } => Self::Bytes,
            ir::Type::Option(inner) => Self::Option(Box::new(Self::of(inner))),
            ir::Type::Vec(inner) => Self::Vec(Box::new(Self::of(inner))),
            ir::Type::Map { key, value } => Self::Map {
                key: Box::new(Self::of(key)),
                value: Box::new(Self::of(value)),
            },
            ir::Type::Named(name) => Self::Record(name.clone()),
            // Streams never reach the mirror model: `Model::new` rejects
            // them first (the JVM async surface is issue #2083).
            ir::Type::Stream(_) => unreachable!("streams are rejected by Model::new"),
        }
    }

    /// Whether the mirror passes by value at the C boundary.
    #[must_use]
    pub const fn is_scalar(&self) -> bool {
        matches!(self, Self::Bool | Self::Int(_) | Self::Float(_))
    }

    /// A name unique per mirror, naming generated Java helpers and keying
    /// deduplicated layout assertions.
    #[must_use]
    pub fn mangle(&self) -> String {
        match self {
            Self::Bool => "Bool".to_owned(),
            Self::Int(kind) => int_mangle(*kind).to_owned(),
            Self::Float(ir::FloatKind::F32) => "F32".to_owned(),
            Self::Float(ir::FloatKind::F64) => "F64".to_owned(),
            Self::Str => "Str".to_owned(),
            Self::Path => "Path".to_owned(),
            Self::Bytes => "Bytes".to_owned(),
            Self::Option(inner) => format!("Opt{}", inner.mangle()),
            Self::Vec(inner) => format!("List{}", inner.mangle()),
            Self::Map { key, value } => {
                format!("Map{}{}", key.mangle(), value.mangle())
            }
            Self::Record(name) => name.clone(),
        }
    }
}

const fn int_mangle(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 => "I8",
        ir::IntKind::I16 => "I16",
        ir::IntKind::I32 => "I32",
        ir::IntKind::I64 => "I64",
        ir::IntKind::Isize => "Isize",
        ir::IntKind::U8 => "U8",
        ir::IntKind::U16 => "U16",
        ir::IntKind::U32 => "U32",
        ir::IntKind::U64 => "U64",
        ir::IntKind::Usize => "Usize",
    }
}

/// Size and alignment of one mirror.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// Bytes, rounded up to the alignment.
    pub size: u64,
    /// Bytes; at most 8.
    pub align: u64,
}

/// A struct layout: the whole struct plus per-field offsets.
#[derive(Debug, Clone)]
pub struct StructLayout {
    /// Size and alignment of the struct itself.
    pub layout: Layout,
    /// Field offsets in declaration order.
    pub offsets: Vec<u64>,
}

/// Layout of one function's return envelope.
#[derive(Debug, Clone)]
pub struct EnvelopeLayout {
    /// Size and alignment of the envelope struct.
    pub layout: Layout,
    /// Offset of the `err_msg` text (the `code` field is always at 0).
    pub err_msg_offset: u64,
    /// Offset of the success payload; `None` for unit-returning functions.
    pub value_offset: Option<u64>,
}

/// `#[repr(C)]` placement: each field starts at the next multiple of its
/// alignment, the struct alignment is the maximum field alignment, and the
/// size rounds up to that alignment.
pub(crate) fn struct_layout(fields: &[Layout]) -> StructLayout {
    let mut cursor = 0_u64;
    let mut align = 1_u64;
    let mut offsets = Vec::with_capacity(fields.len());
    for field in fields {
        let start = cursor.next_multiple_of(field.align);
        offsets.push(start);
        cursor = start + field.size;
        align = align.max(field.align);
    }
    StructLayout {
        layout: Layout {
            size: cursor.next_multiple_of(align),
            align,
        },
        offsets,
    }
}

pub(crate) const fn int_layout(kind: ir::IntKind) -> Layout {
    let size = match kind {
        ir::IntKind::I8 | ir::IntKind::U8 => 1,
        ir::IntKind::I16 | ir::IntKind::U16 => 2,
        ir::IntKind::I32 | ir::IntKind::U32 => 4,
        ir::IntKind::I64 | ir::IntKind::U64 | ir::IntKind::Isize | ir::IntKind::Usize => 8,
    };
    Layout { size, align: size }
}
