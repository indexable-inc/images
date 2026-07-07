//! Classify IR types into what the bridge passes directly and what crosses
//! as an opaque box handle.
//!
//! swift-bridge's type grammar is textual and closed: primitives, `String`,
//! `Vec<primitive>`, and `Option` of those pass straight through, while
//! maps, vectors of non-primitives, options of composites, and records do
//! not. Every type in the second group crosses as an opaque handle -- a
//! newtype in the glue module with index accessors -- and the Swift overlay
//! rebuilds the native value on the other side.

use std::collections::BTreeMap;

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

/// How one IR type crosses the bridge.
pub enum Repr {
    /// A scalar the bridge passes by value.
    Scalar(Scalar),
    /// UTF-8 text (`String`, and paths carried as text).
    Str,
    /// Binary data as `Vec<u8>`.
    Bytes,
    /// A vector of scalars.
    VecScalar(Scalar),
    /// An optional scalar.
    OptionScalar(Scalar),
    /// Optional text (including optional paths).
    OptionStr,
    /// An opaque handle: a record, or a container the bridge grammar cannot
    /// spell.
    Boxed(BoxShape),
}

/// A bridge-native scalar.
#[derive(Clone, Copy)]
pub enum Scalar {
    Bool,
    Int(ir::IntKind),
    Float(ir::FloatKind),
}

/// The value behind an opaque handle.
#[derive(Clone)]
pub enum BoxShape {
    /// A `#[unibind::record]`, held whole behind per-field accessors.
    Record(String),
    /// A vector whose element the bridge cannot spell.
    Vec(ir::Type),
    /// An option whose payload the bridge cannot spell.
    Option(ir::Type),
    /// Any map (swift-bridge has no map type).
    Map { key: ir::Type, value: ir::Type },
    /// A single-value carrier for a throwing function's Ok type that the
    /// bridge could pass directly but cannot NAME in a `Result` (swift-bridge
    /// derives the FFI struct name from the Ok type and its
    /// `to_alpha_numeric_underscore_name` has no arm for i64/u64, `Vec`, or
    /// `Option`, so those `Result`s panic its codegen).
    Value(ir::Type),
}

pub fn repr_of(ty: &ir::Type) -> Repr {
    if let Some(scalar) = scalar_of(ty) {
        return Repr::Scalar(scalar);
    }
    match ty {
        ir::Type::String { .. } | ir::Type::Path { .. } => Repr::Str,
        ir::Type::Bytes { .. } => Repr::Bytes,
        ir::Type::Vec(inner) => scalar_of(inner).map_or_else(
            || Repr::Boxed(BoxShape::Vec((**inner).clone())),
            Repr::VecScalar,
        ),
        ir::Type::Option(inner) => match (scalar_of(inner), &**inner) {
            (Some(scalar), _) => Repr::OptionScalar(scalar),
            (None, ir::Type::String { .. } | ir::Type::Path { .. }) => Repr::OptionStr,
            (None, _) => Repr::Boxed(BoxShape::Option((**inner).clone())),
        },
        ir::Type::Map { key, value } => Repr::Boxed(BoxShape::Map {
            key: (**key).clone(),
            value: (**value).clone(),
        }),
        ir::Type::Named(name) => Repr::Boxed(BoxShape::Record(name.clone())),
        // Streams are rejected before any type mapping runs
        // (`crate::module::render`), so no repr exists for them.
        ir::Type::Stream(_) => unreachable!("streams are rejected before type mapping"),
        ir::Type::Bool | ir::Type::Int(_) | ir::Type::Float(_) => {
            unreachable!("scalar_of covers scalars")
        }
    }
}

fn scalar_of(ty: &ir::Type) -> Option<Scalar> {
    match ty {
        ir::Type::Bool => Some(Scalar::Bool),
        ir::Type::Int(kind) => Some(Scalar::Int(*kind)),
        ir::Type::Float(kind) => Some(Scalar::Float(*kind)),
        _ => None,
    }
}

/// A deterministic UpperCamel name for a type, used to name box handles
/// (`Vec<String>` becomes `VecOfString`).
pub fn mangle(ty: &ir::Type) -> String {
    match ty {
        ir::Type::Bool => "Bool".to_owned(),
        ir::Type::Int(kind) => format!("{kind:?}"),
        ir::Type::Float(kind) => format!("{kind:?}"),
        ir::Type::String { .. } => "String".to_owned(),
        ir::Type::Path { .. } => "Path".to_owned(),
        ir::Type::Bytes { .. } => "Bytes".to_owned(),
        ir::Type::Option(inner) => format!("OptionOf{}", mangle(inner)),
        ir::Type::Vec(inner) => format!("VecOf{}", mangle(inner)),
        ir::Type::Map { key, value } => format!("MapOf{}To{}", mangle(key), mangle(value)),
        ir::Type::Named(name) => name.clone(),
        ir::Type::Stream(inner) => format!("StreamOf{}", mangle(inner)),
    }
}

impl BoxShape {
    /// The handle's name component (`Row`, `VecOfString`, ...).
    pub fn mangle(&self) -> String {
        match self {
            Self::Record(name) => name.clone(),
            Self::Vec(inner) => format!("VecOf{}", mangle(inner)),
            Self::Option(inner) => format!("OptionOf{}", mangle(inner)),
            Self::Map { key, value } => format!("MapOf{}To{}", mangle(key), mangle(value)),
            Self::Value(inner) => format!("ValueOf{}", mangle(inner)),
        }
    }

    /// The opaque handle type's identifier (`__UnibindVecOfString`).
    pub fn ident(&self) -> Ident {
        Ident::new(&format!("__Unibind{}", self.mangle()), Span::call_site())
    }
}

/// The carrier box for a throwing function's Ok type, when the direct repr
/// would make swift-bridge's `Result` codegen panic (see
/// [`BoxShape::Value`]); `None` when the `Result` is expressible directly.
pub fn throws_ok_box(ret: &ir::Type) -> Option<BoxShape> {
    match repr_of(ret) {
        Repr::Scalar(Scalar::Int(ir::IntKind::I64 | ir::IntKind::U64))
        | Repr::Bytes
        | Repr::VecScalar(_)
        | Repr::OptionScalar(_)
        | Repr::OptionStr => Some(BoxShape::Value(ret.clone())),
        Repr::Scalar(_) | Repr::Str | Repr::Boxed(_) => None,
    }
}

/// Every container box the interface needs, keyed by mangled name so the
/// rendered order is deterministic. Records are not collected here; they
/// render as handles of their own.
pub fn collect_boxes(interface: &ir::Interface) -> BTreeMap<String, BoxShape> {
    let mut boxes = BTreeMap::new();
    let field_types = interface
        .records
        .iter()
        .flat_map(|record| record.fields.iter())
        .map(|field| &field.ty);
    let arg_types = interface
        .functions
        .iter()
        .flat_map(|function| function.args.iter())
        .map(|arg| &arg.ty);
    let ret_types = interface
        .functions
        .iter()
        .filter_map(|function| function.ret.as_ref());
    for ty in field_types.chain(arg_types).chain(ret_types) {
        visit(ty, &mut boxes);
    }
    let throwing_rets = interface
        .functions
        .iter()
        .filter(|function| function.throws.is_some())
        .filter_map(|function| function.ret.as_ref());
    for ret in throwing_rets {
        if let Some(shape) = throws_ok_box(ret) {
            boxes.insert(shape.mangle(), shape);
        }
    }
    boxes
}

fn visit(ty: &ir::Type, boxes: &mut BTreeMap<String, BoxShape>) {
    if let Repr::Boxed(shape) = repr_of(ty)
        && !matches!(shape, BoxShape::Record(_))
    {
        boxes.insert(shape.mangle(), shape);
    }
    match ty {
        ir::Type::Option(inner) | ir::Type::Vec(inner) => visit(inner, boxes),
        ir::Type::Map { key, value } => {
            visit(key, boxes);
            visit(value, boxes);
        }
        ir::Type::Stream(inner) => visit(inner, boxes),
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Bytes { .. }
        | ir::Type::Named(_) => {}
    }
}

/// The type as it is spelled inside the bridge module. swift-bridge parses
/// these token streams TEXTUALLY (`"Vec < u8 >"`, `is_ident("String")`), so
/// the spellings stay bare -- no `::std::` qualification.
pub fn bridge_type(ty: &ir::Type) -> TokenStream {
    match repr_of(ty) {
        Repr::Scalar(scalar) => scalar_tokens(scalar),
        Repr::Str => quote!(String),
        Repr::Bytes => quote!(Vec<u8>),
        Repr::VecScalar(scalar) => {
            let scalar = scalar_tokens(scalar);
            quote!(Vec<#scalar>)
        }
        Repr::OptionScalar(scalar) => {
            let scalar = scalar_tokens(scalar);
            quote!(Option<#scalar>)
        }
        Repr::OptionStr => quote!(Option<String>),
        Repr::Boxed(shape) => {
            let ident = shape.ident();
            quote!(#ident)
        }
    }
}

fn scalar_tokens(scalar: Scalar) -> TokenStream {
    match scalar {
        Scalar::Bool => quote!(bool),
        Scalar::Int(kind) => int_tokens(kind),
        Scalar::Float(ir::FloatKind::F32) => quote!(f32),
        Scalar::Float(ir::FloatKind::F64) => quote!(f64),
    }
}

fn int_tokens(kind: ir::IntKind) -> TokenStream {
    match kind {
        ir::IntKind::I8 => quote!(i8),
        ir::IntKind::I16 => quote!(i16),
        ir::IntKind::I32 => quote!(i32),
        ir::IntKind::I64 => quote!(i64),
        ir::IntKind::Isize => quote!(isize),
        ir::IntKind::U8 => quote!(u8),
        ir::IntKind::U16 => quote!(u16),
        ir::IntKind::U32 => quote!(u32),
        ir::IntKind::U64 => quote!(u64),
        ir::IntKind::Usize => quote!(usize),
    }
}

/// The Rust spelling of a boundary type inside the glue module (owned
/// position). Named types resolve through `super::<user>::`.
pub fn rust_type(ty: &ir::Type, user: &Ident) -> TokenStream {
    match ty {
        ir::Type::Bool => quote!(bool),
        ir::Type::Int(kind) => int_tokens(*kind),
        ir::Type::Float(ir::FloatKind::F32) => quote!(f32),
        ir::Type::Float(ir::FloatKind::F64) => quote!(f64),
        ir::Type::String { .. } => quote!(::std::string::String),
        ir::Type::Path { .. } => quote!(::std::path::PathBuf),
        ir::Type::Bytes { .. } => quote!(::std::vec::Vec<u8>),
        ir::Type::Option(inner) => {
            let inner = rust_type(inner, user);
            quote!(::std::option::Option<#inner>)
        }
        ir::Type::Vec(inner) => {
            let inner = rust_type(inner, user);
            quote!(::std::vec::Vec<#inner>)
        }
        ir::Type::Map { key, value } => {
            let key = rust_type(key, user);
            let value = rust_type(value, user);
            quote!(::std::collections::HashMap<#key, #value>)
        }
        ir::Type::Named(name) => {
            let name = Ident::new(name, Span::call_site());
            quote!(super::#user::#name)
        }
        ir::Type::Stream(_) => unreachable!("streams are rejected before type mapping"),
    }
}

/// Convert an owned value of the IR type into its bridge repr.
pub fn to_repr(ty: &ir::Type, expr: &TokenStream) -> TokenStream {
    match ty {
        ir::Type::Path { .. } => quote!(#expr.to_string_lossy().into_owned()),
        ir::Type::Option(inner) if matches!(&**inner, ir::Type::Path { .. }) => {
            quote!(#expr.map(|value| value.to_string_lossy().into_owned()))
        }
        _ => match repr_of(ty) {
            Repr::Boxed(shape) => {
                let ident = shape.ident();
                quote!(#ident::from_value(#expr))
            }
            _ => quote!(#expr),
        },
    }
}

/// Convert an owned bridge-repr value back into the (owned) IR type.
pub fn from_repr(ty: &ir::Type, expr: &TokenStream) -> TokenStream {
    match ty {
        ir::Type::Path { .. } => quote!(::std::path::PathBuf::from(#expr)),
        ir::Type::Option(inner) if matches!(&**inner, ir::Type::Path { .. }) => {
            quote!(#expr.map(::std::path::PathBuf::from))
        }
        _ => match repr_of(ty) {
            Repr::Boxed(_) => quote!(#expr.into_value()),
            _ => quote!(#expr),
        },
    }
}
