//! Assemble the glue module and the `rustler::init!` registration.

use quote::format_ident;
use quote::quote;
use unibind_core::ir;

use crate::{error, function, names, object, record, RenderError, RenderedInterface};

/// Render `rustler` glue for one interface.
///
/// `crate_name` is the consuming crate's `CARGO_CRATE_NAME`, used to alias
/// the plain `nif_init` entry the BEAM dlopens onto rustler's
/// crate-prefixed one: rustler's `init!` exports `<crate>_nif_init` and
/// only adds `nif_init` itself when cargo marks the package primary
/// (`CARGO_PRIMARY_PACKAGE`), which a raw rustc replay like nix-cargo-unit
/// never sets. `None` skips the alias (validation-only callers).
///
/// # Errors
///
/// Fails for surface the elixir backend does not implement (data enums,
/// binary payloads, async fns returning streams, async or stream object
/// members, record field renames), and for renames that cannot become
/// identifiers.
pub fn render(
    interface: &ir::Interface,
    crate_name: Option<&str>,
) -> Result<RenderedInterface, RenderError> {
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
    // Inside the glue module on purpose: rustler's generated entry is a
    // private sibling, unreachable from anywhere else.
    let init_alias = crate_name.map(|name| {
        let prefixed = format_ident!("{name}_nif_init");
        quote! {
            #[unsafe(no_mangle)]
            extern "C" fn nif_init() -> *const ::rustler::codegen_runtime::DEF_NIF_ENTRY {
                #prefixed()
            }
        }
    });
    let glue = quote! {
        #[doc(hidden)]
        #[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
        mod #glue_ident {
            #(#errors)*
            #(#objects)*
            #(#wrappers)*
            #demand
            ::rustler::init!(#native_module);
            #init_alias
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
