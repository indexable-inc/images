//! ABI-stable mirror structs for records, and the `From` conversions
//! between a mirror and its plain counterpart.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;

use crate::function::doc_attrs;
use crate::ty::{self, Paths};

/// The mirror struct definition for one record. Emitted verbatim on both
/// sides of the boundary (engine glue and client `abi` module), so stabby's
/// structural report check compares two spellings of the same tokens.
///
/// `no_opt`: stabby asserts at compile time that a stable struct's layout is
/// not larger than the field-reordered optimum, which turns the user's field
/// order into a compile error whenever Rust would have packed it tighter.
/// Mirror layout is generated, not hand-tuned, and both sides emit the same
/// order from the same IR (the report check still verifies it structurally),
/// so the optimality assert buys nothing here and is opted out.
///
/// `module`: stabby's report check compares the declaring module path, and
/// the two sides' mirrors live in different crates, so both pin the same
/// logical namespace instead of `module_path!()`.
pub(crate) fn mirror_struct(record: &ir::Record, paths: &Paths) -> TokenStream {
    let name = ty::name_ident(&record.name);
    let module = &paths.report_module;
    let doc = format!(
        "ABI-stable mirror of `{}`, field for field in declaration order.",
        record.name
    );
    let fields = record.fields.iter().map(|field| {
        let ident = ty::name_ident(&field.name);
        let stable = ty::stable_type(&field.ty, paths);
        let docs = doc_attrs(&field.docs);
        quote! {
            #docs
            pub #ident: #stable,
        }
    });
    quote! {
        #[doc = #doc]
        #[::stabby::stabby(no_opt, module = #module)]
        pub struct #name {
            #(#fields)*
        }
    }
}

/// `From` impls in both directions between the plain record (through
/// `paths.plain`) and its mirror (through `paths.mirror`).
pub(crate) fn mirror_conversions(record: &ir::Record, paths: &Paths) -> TokenStream {
    let name = ty::name_ident(&record.name);
    let plain = &paths.plain;
    let mirror = &paths.mirror;

    let into_mirror = record.fields.iter().map(|field| {
        let ident = ty::name_ident(&field.name);
        let converted = ty::to_stable(&quote!(value.#ident), &field.ty, paths);
        quote!(#ident: #converted,)
    });
    let into_plain = record.fields.iter().map(|field| {
        let ident = ty::name_ident(&field.name);
        let converted = ty::to_plain(&quote!(value.#ident), &field.ty, paths);
        quote!(#ident: #converted,)
    });
    quote! {
        impl ::core::convert::From<#plain #name> for #mirror #name {
            fn from(value: #plain #name) -> Self {
                Self {
                    #(#into_mirror)*
                }
            }
        }
        impl ::core::convert::From<#mirror #name> for #plain #name {
            fn from(value: #mirror #name) -> Self {
                Self {
                    #(#into_plain)*
                }
            }
        }
    }
}

/// The client's idiomatic record: plain owned fields, `Clone`/`Debug`/
/// `PartialEq` so consumers can assert round trips.
pub(crate) fn plain_record(record: &ir::Record, paths: &Paths) -> TokenStream {
    let name = ty::name_ident(&record.name);
    let docs = fallback_docs(&record.docs, &format!("The `{}` record.", record.name));
    let fields = record.fields.iter().map(|field| {
        let ident = ty::name_ident(&field.name);
        let plain = ty::plain_type(&field.ty, paths);
        let docs = fallback_docs(&field.docs, &format!("The `{}` field.", field.name));
        quote! {
            #docs
            pub #ident: #plain,
        }
    });
    quote! {
        #docs
        #[derive(Clone, Debug, PartialEq)]
        pub struct #name {
            #(#fields)*
        }
    }
}

/// IR docs, or a generated one-liner when the user wrote none, so every
/// public item in the generated crate is documented.
pub(crate) fn fallback_docs(lines: &[String], fallback: &str) -> TokenStream {
    if lines.is_empty() {
        quote!(#[doc = #fallback])
    } else {
        doc_attrs(lines)
    }
}
