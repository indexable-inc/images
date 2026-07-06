//! Assemble the glue module and the `#[pymodule]` registration.

use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::{error, function, record, ty, RenderError, RenderedInterface};

/// Render `pyo3` glue for one interface.
///
/// # Errors
///
/// Fails for surface the phase 0 backend does not implement (async
/// functions, data enums, objects), for unsupported `py(base = ...)`
/// exception bases, and for renames that cannot become identifiers.
pub fn render(interface: &ir::Interface) -> Result<RenderedInterface, RenderError> {
    if let Some(object) = interface.objects.first() {
        return Err(RenderError::new(format!(
            "`{}` is a #[unibind::object]; objects land in phase 2 (issue #1992)",
            object.name
        )));
    }
    if let Some(data_enum) = interface.enums.first() {
        return Err(RenderError::new(format!(
            "`{}` is a data enum, which phase 0 does not render",
            data_enum.name
        )));
    }

    let user = ty::name_ident(&interface.name)?;
    let module_name = interface.names.py.clone().unwrap_or_else(|| interface.name.clone());
    let module_ident = ty::name_ident(&module_name)?;
    let glue_ident = format_ident!("__unibind_py_{}", interface.name.trim_start_matches('_'));

    let exceptions = interface
        .errors
        .iter()
        .map(|err| error::render_error(err, &module_ident, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let wrappers = interface
        .functions
        .iter()
        .map(|func| function::render_fn(func, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let constructors = interface
        .records
        .iter()
        .map(|rec| record::constructor(rec, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let registration = registration(interface, &user)?;
    let module_docs = function::doc_attrs(&interface.docs);

    let glue = quote! {
        #[doc(hidden)]
        #[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
        mod #glue_ident {
            use ::pyo3::types::PyModuleMethods as _;

            #(#exceptions)*
            #(#constructors)*
            #(#wrappers)*

            #module_docs
            #[::pyo3::pymodule]
            #[pyo3(name = #module_name)]
            fn __unibind_module(
                module: &::pyo3::Bound<'_, ::pyo3::types::PyModule>,
            ) -> ::pyo3::PyResult<()> {
                #registration
                ::pyo3::PyResult::Ok(())
            }
        }
    };
    let records = interface.records.iter().map(record::record_attrs).collect();
    Ok(RenderedInterface { glue, records })
}

fn registration(interface: &ir::Interface, user: &Ident) -> Result<TokenStream, RenderError> {
    let mut statements = Vec::new();
    for func in &interface.functions {
        let ident = Ident::new(&func.name, Span::call_site());
        statements.push(quote! {
            module.add_function(::pyo3::wrap_pyfunction!(#ident, module)?)?;
        });
    }
    for rec in &interface.records {
        let ident = Ident::new(&rec.name, Span::call_site());
        statements.push(quote! {
            module.add_class::<super::#user::#ident>()?;
        });
    }
    for err in &interface.errors {
        let base_name = err.names.py.as_deref().unwrap_or(&err.name);
        let base_ident = ty::name_ident(base_name)?;
        statements.push(quote! {
            module.add(#base_name, module.py().get_type::<#base_ident>())?;
        });
        for variant in &err.variants {
            let class_name = variant.names.py.as_deref().unwrap_or(&variant.name);
            let class_ident = ty::name_ident(class_name)?;
            statements.push(quote! {
                module.add(#class_name, module.py().get_type::<#class_ident>())?;
            });
        }
    }
    statements.push(quote! {
        module.add("__version__", ::std::env!("CARGO_PKG_VERSION"))?;
    });
    Ok(quote! { #(#statements)* })
}
