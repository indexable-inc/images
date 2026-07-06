//! Render `#[pyfunction]` wrappers around the user's functions.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::{ty, RenderError};

pub fn render_fn(function: &ir::Function, user: &Ident) -> Result<TokenStream, RenderError> {
    if matches!(function.asyncness, ir::Asyncness::Async) {
        return Err(RenderError::new(format!(
            "`{}` is async; async functions land in phase 2 (issue #1992)",
            function.name
        )));
    }
    let rust_name = Ident::new(&function.name, Span::call_site());
    let rename = function.names.py.as_ref().map(|name| {
        quote! { #[pyo3(name = #name)] }
    });
    let docs = doc_attrs(&function.docs);

    let mut params = Vec::new();
    let mut signature = Vec::new();
    let mut forwarded = Vec::new();
    for arg in &function.args {
        let ident = ty::name_ident(arg.names.py.as_ref().unwrap_or(&arg.name))?;
        let ty = ty::rust_type(&arg.ty, user);
        params.push(quote!(#ident: #ty));
        signature.push(signature_entry(arg, &ident));
        forwarded.push(quote!(#ident));
    }

    let ok_ty = function
        .ret
        .as_ref()
        .map_or_else(|| quote!(()), |ret| ty::rust_type(ret, user));
    let call = quote!(super::#user::#rust_name(#(#forwarded),*));
    let body_and_ret = if function.throws.is_some() {
        BodyAndRet {
            ret: quote!(::pyo3::PyResult<#ok_ty>),
            body: quote!(#call.map_err(::pyo3::PyErr::from)),
        }
    } else {
        BodyAndRet {
            ret: ok_ty,
            body: call,
        }
    };
    let BodyAndRet { ret, body } = body_and_ret;

    Ok(quote! {
        #docs
        #[::pyo3::pyfunction]
        #rename
        #[pyo3(signature = (#(#signature),*))]
        fn #rust_name(#(#params),*) -> #ret {
            #body
        }
    })
}

/// A wrapper's return type and body, which vary together on `throws`.
struct BodyAndRet {
    ret: TokenStream,
    body: TokenStream,
}

fn signature_entry(arg: &ir::Arg, ident: &Ident) -> TokenStream {
    if let Some(default) = &arg.default {
        let default = ty::default_tokens(default);
        return quote!(#ident = #default);
    }
    if matches!(arg.ty, ir::Type::Option(_)) {
        return quote!(#ident = None);
    }
    quote!(#ident)
}

pub fn doc_attrs(lines: &[String]) -> TokenStream {
    quote! { #(#[doc = #lines])* }
}
