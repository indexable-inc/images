//! Render error enums as exception hierarchies.
//!
//! One `create_exception!` base class per enum (extending the requested
//! Python built-in), one subclass per variant, and a `From<Enum> for PyErr`
//! impl that picks the subclass and carries the enum's `Display` text.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::function::doc_attrs;
use crate::RenderError;

pub fn render_error(
    error: &ir::ErrorType,
    module_name: &Ident,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    let rust_name = Ident::new(&error.name, Span::call_site());
    let base_class = Ident::new(error.names.py.as_deref().unwrap_or(&error.name), Span::call_site());
    let builtin = builtin_base(error.py_base.as_deref())?;
    let base_doc = error.docs.join("\n");

    let mut classes = vec![quote! {
        ::pyo3::create_exception!(#module_name, #base_class, #builtin, #base_doc);
    }];
    let mut arms = Vec::new();
    for variant in &error.variants {
        let variant_ident = Ident::new(&variant.name, Span::call_site());
        let class = Ident::new(
            variant.names.py.as_deref().unwrap_or(&variant.name),
            Span::call_site(),
        );
        let doc = variant.docs.join("\n");
        classes.push(quote! {
            ::pyo3::create_exception!(#module_name, #class, #base_class, #doc);
        });
        arms.push(quote! {
            super::#user::#rust_name::#variant_ident { .. } => #class::new_err(message),
        });
    }

    let from_docs = doc_attrs(&[format!(
        "Map `{}` onto its exception class, message from `Display`.",
        error.name
    )]);
    Ok(quote! {
        #(#classes)*
        #from_docs
        impl ::std::convert::From<super::#user::#rust_name> for ::pyo3::PyErr {
            fn from(error: super::#user::#rust_name) -> Self {
                let message = ::std::string::ToString::to_string(&error);
                match error {
                    #(#arms)*
                }
            }
        }
    })
}

/// Exception classes the base can extend, by Python name.
fn builtin_base(name: Option<&str>) -> Result<TokenStream, RenderError> {
    let path = match name.unwrap_or("Exception") {
        "Exception" => quote!(::pyo3::exceptions::PyException),
        "ValueError" => quote!(::pyo3::exceptions::PyValueError),
        "TypeError" => quote!(::pyo3::exceptions::PyTypeError),
        "RuntimeError" => quote!(::pyo3::exceptions::PyRuntimeError),
        "OSError" => quote!(::pyo3::exceptions::PyOSError),
        "IOError" => quote!(::pyo3::exceptions::PyIOError),
        "KeyError" => quote!(::pyo3::exceptions::PyKeyError),
        "LookupError" => quote!(::pyo3::exceptions::PyLookupError),
        "NotImplementedError" => quote!(::pyo3::exceptions::PyNotImplementedError),
        other => {
            return Err(RenderError::new(format!(
                "`py(base = \"{other}\")` is not a supported base exception; use one of \
                 Exception, ValueError, TypeError, RuntimeError, OSError, IOError, \
                 KeyError, LookupError, NotImplementedError"
            )));
        }
    };
    Ok(path)
}
