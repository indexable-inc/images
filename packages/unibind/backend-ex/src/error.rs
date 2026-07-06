//! Render error enums as BEAM-friendly structs.
//!
//! Rustler has no exception hierarchy to mirror pyo3's `create_exception!`,
//! so an error crosses as data: one `%<Ns>.<Error>{variant: atom, message:
//! String.t()}` struct per enum, built by a `From` impl that snake-cases
//! the variant and carries the enum's `Display` text. Functions with a
//! `throws` return `Result<_, <Error>Term>`, which rustler encodes as the
//! idiomatic `{:ok, value} | {:error, error}`.

use heck::ToSnakeCase as _;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::{names, RenderError};

/// The glue-module struct name carrying `error` over the boundary.
pub fn term_ident(error_name: &str) -> Ident {
    format_ident!("{error_name}Term")
}

pub fn render_error(
    error: &ir::ErrorType,
    ns: &str,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    let rust_name = Ident::new(&error.name, Span::call_site());
    let term_name = term_ident(&error.name);
    let module = format!("{ns}.{}", names::ex_error_name(error));
    let atoms_module = format_ident!("__unibind_atoms_{}", error.name.to_snake_case());

    let mut atom_defs = Vec::new();
    let mut arms = Vec::new();
    for variant in &error.variants {
        let variant_ident = Ident::new(&variant.name, Span::call_site());
        let atom = names::name_ident(&names::variant_atom(variant))?;
        atom_defs.push(quote!(#atom));
        arms.push(quote! {
            super::#user::#rust_name::#variant_ident { .. } => #atoms_module::#atom(),
        });
    }

    let from_docs = format!(
        "Carry `{}` across the boundary: variant atom plus `Display` text.",
        error.name
    );
    Ok(quote! {
        mod #atoms_module {
            ::rustler::atoms! {
                #(#atom_defs),*
            }
        }
        #[doc = #from_docs]
        #[derive(::rustler::NifStruct)]
        #[module = #module]
        pub struct #term_name {
            variant: ::rustler::types::atom::Atom,
            message: ::std::string::String,
        }
        impl ::std::convert::From<super::#user::#rust_name> for #term_name {
            fn from(error: super::#user::#rust_name) -> Self {
                let message = ::std::string::ToString::to_string(&error);
                let variant = match &error {
                    #(#arms)*
                };
                Self { variant, message }
            }
        }
    })
}
