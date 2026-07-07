//! Assemble the glue module and the `rustler::init!` registration.

use quote::format_ident;
use quote::quote;
use unibind_core::ir;

use crate::{error, function, names, object, record, RenderError, RenderedInterface};

/// Render `rustler` glue for one interface.
///
/// # Errors
///
/// Fails for surface the elixir backend does not implement (data enums,
/// binary payloads, async fns returning streams, async or stream object
/// members, record field renames), and for renames that cannot become
/// identifiers.
pub fn render(interface: &ir::Interface) -> Result<RenderedInterface, RenderError> {
    if let Some(data_enum) = interface.enums.first() {
        return Err(RenderError::new(format!(
            "`{}` is a data enum, which the elixir backend does not render",
            data_enum.name
        )));
    }

    let user = names::name_ident(&interface.name)?;
    let ns = names::ns_name(interface);
    let glue_ident = format_ident!("__unibind_ex_{}", interface.name.trim_start_matches('_'));

    let errors = interface
        .errors
        .iter()
        .map(|err| error::render_error(err, &ns, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let objects = interface
        .objects
        .iter()
        .map(|obj| object::render_object(obj, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let wrappers = interface
        .functions
        .iter()
        .map(|func| function::render_fn(func, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let demand = has_streams(interface).then(function::demand_nif);
    let records = interface
        .records
        .iter()
        .map(|rec| record::record_attrs(rec, &ns))
        .collect::<Result<Vec<_>, _>>()?;

    let native_module = format!("Elixir.{ns}.Native");
    let glue = quote! {
        #[doc(hidden)]
        #[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
        mod #glue_ident {
            #(#errors)*
            #(#objects)*
            #(#wrappers)*
            #demand
            ::rustler::init!(#native_module);
        }
    };
    Ok(RenderedInterface { glue, records })
}

/// Whether any free function returns a stream (object members cannot yet).
pub fn has_streams(interface: &ir::Interface) -> bool {
    interface
        .functions
        .iter()
        .any(|function| matches!(function.ret, Some(ir::Type::Stream(_))))
}
