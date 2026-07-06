//! Expansion pipeline: parse once, lower to IR, dispatch to backends.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::LowerError;

pub fn export(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut module = match syn::parse2::<syn::ItemMod>(item.clone()) {
        Ok(module) => module,
        Err(error) => {
            let error = error.to_compile_error();
            return quote! { #item #error };
        }
    };
    let interface = match unibind_core::lower_module(args, &module) {
        Ok(interface) => interface,
        Err(error) => return with_error(&mut module, &error),
    };
    unibind_core::strip_unibind_attrs(&mut module);
    let embed = match unibind_core::embed::ir_static(&interface) {
        Ok(embed) => embed,
        Err(error) => return with_error(&mut module, &error),
    };
    let glue = match backends(&interface, &mut module) {
        Ok(glue) => glue,
        Err(error) => return with_error(&mut module, &error),
    };
    quote! {
        #module
        #embed
        #glue
    }
}

/// Emit the module (markers stripped, so nothing cascades) plus the
/// positioned diagnostic.
fn with_error(module: &mut syn::ItemMod, error: &LowerError) -> TokenStream {
    unibind_core::strip_unibind_attrs(module);
    let error = syn::Error::new(error.span, &error.message).to_compile_error();
    quote! { #module #error }
}

#[cfg(feature = "py")]
fn backends(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
) -> Result<TokenStream, LowerError> {
    let rendered = unibind_backend_py::render(interface).map_err(|error| LowerError {
        span: proc_macro2::Span::call_site(),
        message: error.message,
    })?;
    splice_record_attrs(interface, module, &rendered);
    Ok(rendered.glue)
}

/// With no backend feature enabled the macro still validates the surface
/// and embeds the IR; there is just no binding code to add.
#[cfg(not(feature = "py"))]
fn backends(
    _interface: &unibind_core::ir::Interface,
    _module: &mut syn::ItemMod,
) -> Result<TokenStream, LowerError> {
    Ok(TokenStream::new())
}

/// Attach the backend's `#[pyclass]`-shaped attributes to the record
/// structs the IR was lowered from. Records and rendered attribute sets are
/// index-aligned by construction.
#[cfg(feature = "py")]
fn splice_record_attrs(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    rendered: &unibind_backend_py::RenderedInterface,
) {
    let Some((_, items)) = &mut module.content else {
        return;
    };
    for (record, attrs) in interface.records.iter().zip(&rendered.records) {
        for item in &mut *items {
            let syn::Item::Struct(item) = item else {
                continue;
            };
            if item.ident != record.name {
                continue;
            }
            let mut outer = attrs.outer.clone();
            outer.append(&mut item.attrs);
            item.attrs = outer;
            for (field, field_attrs) in item.fields.iter_mut().zip(&attrs.fields) {
                field.attrs.extend(field_attrs.iter().cloned());
            }
        }
    }
}

pub fn marker_outside_export(item: TokenStream, message: &str) -> TokenStream {
    let error = syn::Error::new(proc_macro2::Span::call_site(), message).to_compile_error();
    quote! { #item #error }
}
