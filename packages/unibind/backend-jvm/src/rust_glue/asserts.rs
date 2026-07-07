//! `const` assertions pinning every mirror layout to the computed numbers.
//!
//! The Java generator bakes the same numbers into its reads and writes, so
//! a Rust compiler that lays a mirror out differently fails the user's
//! build instead of corrupting memory at runtime.

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::ctype::CTy;
use crate::model::Model;
use crate::names;
use crate::rust_glue::types::{envelope_ident, item_envelope_ident, mirror_tokens};
use crate::rust_glue::Export;

/// The `const _: () = { ... }` block checking every reachable aggregate,
/// every export envelope, and every stream export's item envelope.
pub fn layout_asserts(
    interface: &ir::Interface,
    model: &Model<'_>,
    exports: &[Export<'_>],
) -> TokenStream {
    let mut checks = Vec::new();

    let roots = exports
        .iter()
        .flat_map(|export| {
            export
                .function
                .args
                .iter()
                .map(|arg| &arg.ty)
                .chain(export.ret.as_ref())
        })
        .chain(
            interface
                .records
                .iter()
                .flat_map(|record| record.fields.iter().map(|field| &field.ty)),
        );
    let mut aggregates = model.reachable_aggregates(roots);
    // `err_msg` makes the text mirror reachable from every envelope.
    aggregates.entry(CTy::Str.mangle()).or_insert(CTy::Str);

    for ty in aggregates.values() {
        checks.push(size_align(&mirror_tokens(ty), model.layout(ty).size, model.layout(ty).align));
        checks.push(field_checks(ty, model));
    }

    for export in exports {
        let ident = envelope_ident(export.owner, &export.function.name);
        let ret = export.ret.as_ref().map(|ty| model.boundary(ty));
        checks.push(envelope_checks(&quote!(#ident), model, ret.as_ref()));
        if let Some(ir::Type::Stream(item)) = &export.ret {
            let item_ident = item_envelope_ident(export.owner, &export.function.name);
            let payload = CTy::Option(Box::new(CTy::of(item)));
            checks.push(envelope_checks(&quote!(#item_ident), model, Some(&payload)));
        }
    }

    quote! {
        const _: () = {
            #(#checks)*
        };
    }
}

/// Size, alignment, and field offsets of one envelope-shaped struct.
fn envelope_checks(tokens: &TokenStream, model: &Model<'_>, ret: Option<&CTy>) -> TokenStream {
    let envelope = model.envelope(ret);
    let mut checks = vec![size_align(tokens, envelope.layout.size, envelope.layout.align)];
    let err_msg = Literal::u64_unsuffixed(envelope.err_msg_offset);
    checks.push(quote! {
        assert!(::core::mem::offset_of!(#tokens, code) == 0);
        assert!(::core::mem::offset_of!(#tokens, err_msg) == #err_msg);
    });
    if let Some(value_offset) = envelope.value_offset {
        let value = Literal::u64_unsuffixed(value_offset);
        checks.push(quote! {
            assert!(::core::mem::offset_of!(#tokens, value) == #value);
        });
    }
    quote!(#(#checks)*)
}

fn size_align(tokens: &TokenStream, size: u64, align: u64) -> TokenStream {
    let size = Literal::u64_unsuffixed(size);
    let align = Literal::u64_unsuffixed(align);
    quote! {
        assert!(::core::mem::size_of::<#tokens>() == #size);
        assert!(::core::mem::align_of::<#tokens>() == #align);
    }
}

fn field_checks(ty: &CTy, model: &Model<'_>) -> TokenStream {
    let tokens = mirror_tokens(ty);
    match ty {
        CTy::Str | CTy::Path | CTy::Bytes | CTy::Vec(_) => quote! {
            assert!(::core::mem::offset_of!(#tokens, ptr) == 0);
            assert!(::core::mem::offset_of!(#tokens, len) == 8);
        },
        CTy::Map { key, value } => {
            let pair = model.pair_struct(key, value);
            let key_tokens = mirror_tokens(key);
            let value_tokens = mirror_tokens(value);
            let pair_tokens = quote!(CPair<#key_tokens, #value_tokens>);
            let pair_checks = size_align(&pair_tokens, pair.layout.size, pair.layout.align);
            let key_offset = Literal::u64_unsuffixed(pair.offsets[0]);
            let value_offset = Literal::u64_unsuffixed(pair.offsets[1]);
            quote! {
                assert!(::core::mem::offset_of!(#tokens, ptr) == 0);
                assert!(::core::mem::offset_of!(#tokens, len) == 8);
                #pair_checks
                assert!(::core::mem::offset_of!(#pair_tokens, key) == #key_offset);
                assert!(::core::mem::offset_of!(#pair_tokens, value) == #value_offset);
            }
        }
        CTy::Option(inner) => {
            let value_offset = Literal::u64_unsuffixed(model.option_value_offset(inner));
            quote! {
                assert!(::core::mem::offset_of!(#tokens, present) == 0);
                assert!(::core::mem::offset_of!(#tokens, value) == #value_offset);
            }
        }
        CTy::Record(name) => {
            let shape = model.record_struct(name);
            let mut checks = Vec::new();
            for (field, offset) in model.record(name).fields.iter().zip(&shape.offsets) {
                let Ok(ident) = names::rust_ident(&field.name) else {
                    // The mirror-struct emitter already errored on this name.
                    continue;
                };
                let offset = Literal::u64_unsuffixed(*offset);
                checks.push(quote! {
                    assert!(::core::mem::offset_of!(#tokens, #ident) == #offset);
                });
            }
            quote!(#(#checks)*)
        }
        CTy::Bool | CTy::Int(_) | CTy::Float(_) | CTy::Handle => TokenStream::new(),
    }
}
