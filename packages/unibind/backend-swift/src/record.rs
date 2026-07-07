//! Render each record as an opaque handle: a field-by-field constructor
//! plus per-field getter methods.
//!
//! swift-bridge shared structs cannot hold maps, vectors of non-primitives,
//! or nested records, so records cross as opaque handles instead; the Swift
//! overlay presents a native Swift struct and converts at the boundary.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::names;
use crate::repr::{self, BoxShape};
use crate::RenderError;

/// One rendered record handle: bridge declarations plus glue items.
pub struct RenderedRecord {
    pub bridge_decls: TokenStream,
    pub items: TokenStream,
}

/// The handle constructor's name (`__unibind_new_row`).
pub fn ctor_ident(record: &ir::Record) -> Ident {
    let snake = names::to_snake(&record.name);
    Ident::new(&format!("__unibind_new_{snake}"), Span::call_site())
}

/// Names the generated handle claims for itself; a record field with one of
/// these names would collide with the conversion methods.
const RESERVED_FIELD_NAMES: &[&str] = &["from_value", "into_value"];

pub fn render_record(record: &ir::Record, user: &Ident) -> Result<RenderedRecord, RenderError> {
    let shape = BoxShape::Record(record.name.clone());
    let handle = shape.ident();
    let ctor = ctor_ident(record);
    let name = names::name_ident(&record.name)?;

    let mut ctor_params = Vec::new();
    let mut ctor_fields = Vec::new();
    let mut getter_decls = Vec::new();
    let mut getters = Vec::new();
    for field in &record.fields {
        if RESERVED_FIELD_NAMES.contains(&field.name.as_str()) {
            return Err(RenderError::new(format!(
                "`{}.{}` collides with the generated handle's conversion \
                 methods; rename the field",
                record.name, field.name
            )));
        }
        let ident = names::name_ident(&field.name)?;
        let bridge_ty = repr::bridge_type(&field.ty);
        let param = quote!(#ident);
        let from = repr::from_repr(&field.ty, &param);
        ctor_params.push(quote!(#ident: #bridge_ty));
        ctor_fields.push(quote!(#ident: #from));
        getter_decls.push(quote! {
            fn #ident(self: &#handle) -> #bridge_ty;
        });
        let cloned = quote!(self.0.#ident.clone());
        let read = repr::to_repr(&field.ty, &cloned);
        getters.push(quote! {
            fn #ident(&self) -> #bridge_ty {
                #read
            }
        });
    }

    let bridge_decls = quote! {
        type #handle;
        fn #ctor(#(#ctor_params),*) -> #handle;
        #(#getter_decls)*
    };
    let items = quote! {
        pub struct #handle(super::#user::#name);
        impl #handle {
            fn from_value(value: super::#user::#name) -> Self {
                Self(value)
            }
            fn into_value(self) -> super::#user::#name {
                self.0
            }
            #(#getters)*
        }
        fn #ctor(#(#ctor_params),*) -> #handle {
            #handle(super::#user::#name {
                #(#ctor_fields),*
            })
        }
    };
    Ok(RenderedRecord {
        bridge_decls,
        items,
    })
}
