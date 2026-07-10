//! Wire codec expressions for the generated Rust glue, and validation that
//! a type is representable on the JVM boundary.

use heck::ToSnakeCase as _;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::RenderError;

/// Reject the types the jvm backend cannot carry across the boundary.
/// Streams have no synchronous C-ABI shape; a `Named` type must be a
/// record, since objects do not cross (there is no handle registry yet).
pub fn check_boundary(ty: &ir::Type, interface: &ir::Interface) -> Result<(), RenderError> {
    match ty {
        ir::Type::Stream(_) => Err(RenderError::new(
            "`Stream<T>` is not part of the jvm backend yet; return a \
             `Vec<T>` instead",
        )),
        ir::Type::Named(name) => {
            if interface.records.iter().any(|record| record.name == *name) {
                Ok(())
            } else {
                Err(RenderError::new(format!(
                    "`{name}` is not a `#[unibind::record]`; only records \
                     cross the jvm boundary by value"
                )))
            }
        }
        ir::Type::Option(inner) => {
            if matches!(**inner, ir::Type::Option(_)) {
                return Err(RenderError::new(
                    "`Option<Option<T>>` cannot cross the jvm boundary: Java \
                     spells `None` as `null`, which cannot carry the inner \
                     `None`; flatten the option or wrap it in a record",
                ));
            }
            check_boundary(inner, interface)
        }
        ir::Type::Vec(inner) => check_boundary(inner, interface),
        ir::Type::Map { key, value } => {
            check_boundary(key, interface)?;
            check_boundary(value, interface)
        }
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Bytes { .. } => Ok(()),
    }
}

/// The glue-module identifier of a record's decode helper.
pub fn read_record_ident(record_name: &str) -> Ident {
    format_ident!("__read_{}", record_name.to_snake_case())
}

/// The glue-module identifier of a record's encode helper.
pub fn write_record_ident(record_name: &str) -> Ident {
    format_ident!("__write_{}", record_name.to_snake_case())
}

/// The `Reader` method decoding one integer kind.
const fn read_int_method(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 => "read_i8",
        ir::IntKind::I16 => "read_i16",
        ir::IntKind::I32 => "read_i32",
        ir::IntKind::I64 => "read_i64",
        ir::IntKind::Isize => "read_isize",
        ir::IntKind::U8 => "read_u8",
        ir::IntKind::U16 => "read_u16",
        ir::IntKind::U32 => "read_u32",
        ir::IntKind::U64 => "read_u64",
        ir::IntKind::Usize => "read_usize",
    }
}

/// The `Writer` method encoding one integer kind.
const fn write_int_method(kind: ir::IntKind) -> &'static str {
    match kind {
        ir::IntKind::I8 => "write_i8",
        ir::IntKind::I16 => "write_i16",
        ir::IntKind::I32 => "write_i32",
        ir::IntKind::I64 => "write_i64",
        ir::IntKind::Isize => "write_isize",
        ir::IntKind::U8 => "write_u8",
        ir::IntKind::U16 => "write_u16",
        ir::IntKind::U32 => "write_u32",
        ir::IntKind::U64 => "write_u64",
        ir::IntKind::Usize => "write_usize",
    }
}

/// An expression decoding one value of `ty` from `reader`, spelled at the
/// type the user's signature declares (borrows decode zero-copy from the
/// payload). `depth` uniquifies the loop bindings of nested containers.
pub fn decode_expr(ty: &ir::Type, depth: usize) -> TokenStream {
    match ty {
        ir::Type::Bool => quote!(reader.read_bool()),
        ir::Type::Int(kind) => {
            let method = Ident::new(read_int_method(*kind), Span::call_site());
            quote!(reader.#method())
        }
        ir::Type::Float(ir::FloatKind::F32) => quote!(reader.read_f32()),
        ir::Type::Float(ir::FloatKind::F64) => quote!(reader.read_f64()),
        ir::Type::String { owned: false } => quote!(reader.read_str()),
        ir::Type::String { owned: true } => quote!(reader.read_string()),
        ir::Type::Path { owned: false } => quote!(::std::path::Path::new(reader.read_str())),
        ir::Type::Path { owned: true } => {
            quote!(::std::path::PathBuf::from(reader.read_string()))
        }
        ir::Type::Bytes { owned: false } => quote!(reader.read_bytes()),
        ir::Type::Bytes { owned: true } => quote!(reader.read_byte_buf()),
        ir::Type::Option(inner) => {
            let inner = decode_expr(inner, depth);
            quote! {
                if reader.read_bool() {
                    ::std::option::Option::Some(#inner)
                } else {
                    ::std::option::Option::None
                }
            }
        }
        ir::Type::Vec(inner) => {
            let count = format_ident!("__count{depth}");
            let items = format_ident!("__items{depth}");
            let inner = decode_expr(inner, depth + 1);
            quote! {
                {
                    let #count = reader.read_count();
                    let mut #items = ::std::vec::Vec::with_capacity(#count);
                    for _ in 0..#count {
                        #items.push(#inner);
                    }
                    #items
                }
            }
        }
        ir::Type::Map { key, value } => {
            let count = format_ident!("__count{depth}");
            let entries = format_ident!("__entries{depth}");
            let key_binding = format_ident!("__key{depth}");
            let key = decode_expr(key, depth + 1);
            let value = decode_expr(value, depth + 1);
            quote! {
                {
                    let #count = reader.read_count();
                    let mut #entries = ::std::collections::HashMap::with_capacity(#count);
                    for _ in 0..#count {
                        let #key_binding = #key;
                        #entries.insert(#key_binding, #value);
                    }
                    #entries
                }
            }
        }
        ir::Type::Named(name) => {
            let read = read_record_ident(name);
            quote!(#read(reader))
        }
        // Rejected by `check_boundary` before anything is spelled.
        ir::Type::Stream(_) => unreachable!("rejected by check_boundary"),
    }
}

/// Statements encoding the value at place expression `place` (of type `ty`)
/// into `writer`. `depth` uniquifies the bindings of nested containers.
pub fn encode_stmts(ty: &ir::Type, place: &TokenStream, depth: usize) -> TokenStream {
    match ty {
        ir::Type::Bool => quote!(writer.write_bool(#place);),
        ir::Type::Int(kind) => {
            let method = Ident::new(write_int_method(*kind), Span::call_site());
            quote!(writer.#method(#place);)
        }
        ir::Type::Float(ir::FloatKind::F32) => quote!(writer.write_f32(#place);),
        ir::Type::Float(ir::FloatKind::F64) => quote!(writer.write_f64(#place);),
        ir::Type::String { .. } => quote!(writer.write_str(&#place);),
        // Paths cross as strings; the JVM has no byte-path type.
        ir::Type::Path { .. } => quote! {
            writer.write_str(
                ::std::path::Path::to_str(#place.as_ref())
                    .expect("non-UTF-8 path crossing the jvm boundary"),
            );
        },
        ir::Type::Bytes { .. } => quote!(writer.write_bytes(&#place);),
        ir::Type::Option(inner) => {
            let binding = format_ident!("__some{depth}");
            let inner = encode_stmts(inner, &quote!((*#binding)), depth + 1);
            quote! {
                match &#place {
                    ::std::option::Option::Some(#binding) => {
                        writer.write_bool(true);
                        #inner
                    }
                    ::std::option::Option::None => {
                        writer.write_bool(false);
                    }
                }
            }
        }
        ir::Type::Vec(inner) => {
            let binding = format_ident!("__item{depth}");
            let inner = encode_stmts(inner, &quote!((*#binding)), depth + 1);
            quote! {
                writer.write_count(#place.len());
                for #binding in &#place {
                    #inner
                }
            }
        }
        ir::Type::Map { key, value } => {
            let key_binding = format_ident!("__key{depth}");
            let value_binding = format_ident!("__value{depth}");
            let key = encode_stmts(key, &quote!((*#key_binding)), depth + 1);
            let value = encode_stmts(value, &quote!((*#value_binding)), depth + 1);
            quote! {
                writer.write_count(#place.len());
                for (#key_binding, #value_binding) in &#place {
                    #key
                    #value
                }
            }
        }
        ir::Type::Named(name) => {
            let write = write_record_ident(name);
            quote!(#write(writer, &#place);)
        }
        // Rejected by `check_boundary` before anything is spelled.
        ir::Type::Stream(_) => unreachable!("rejected by check_boundary"),
    }
}
