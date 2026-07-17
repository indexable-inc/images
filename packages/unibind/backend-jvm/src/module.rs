//! Assemble the hidden glue module: record codecs, error mappers, one
//! `extern "C"` shim per function, and the buffer-free symbol.

use quote::{format_ident, quote};
use unibind_core::ir;

use crate::{RenderError, RenderedInterface, error, function, names, record};

/// Render the C-ABI glue for one interface.
///
/// # Errors
///
/// Fails for surface the jvm backend does not implement (data enums,
/// objects, async fns, streams), for `jvm(base = ...)` values outside the
/// supported Java exceptions, and for names Java cannot declare.
pub fn render(interface: &ir::Interface) -> Result<RenderedInterface, RenderError> {
    if let Some(data_enum) = interface.enums.first() {
        return Err(RenderError::new(format!(
            "`{}` is a data enum, which the jvm backend does not render",
            data_enum.name
        )));
    }
    if let Some(object) = interface.objects.first() {
        return Err(RenderError::new(format!(
            "`{}` is an object, which the jvm backend does not render yet; \
             expose free functions instead",
            object.name
        )));
    }

    let user = names::name_ident(&interface.name)?;
    let glue_ident = format_ident!("__unibind_jvm_{}", interface.name.trim_start_matches('_'));

    let codecs = interface
        .records
        .iter()
        .map(|rec| record::render_codecs(rec, interface, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let errors = interface
        .errors
        .iter()
        .map(|err| error::render_error(err, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let shims = interface
        .functions
        .iter()
        .map(|func| function::render_fn(func, interface, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let free = function::render_free(interface);
    let records = interface.records.iter().map(record::record_attrs).collect();

    let glue = quote! {
        #[doc(hidden)]
        #[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
        mod #glue_ident {
            #(#codecs)*
            #(#errors)*
            #(#shims)*
            #free
        }
    };
    Ok(RenderedInterface { glue, records })
}
