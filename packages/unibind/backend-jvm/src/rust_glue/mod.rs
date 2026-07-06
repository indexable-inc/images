//! Assemble the hidden glue module of plain `extern "C"` exports.

mod asserts;
mod decode;
mod encode;
mod function;
mod types;

use quote::{format_ident, quote};
use unibind_core::ir;

use crate::model::Model;
use crate::{names, RenderError, RenderedJvm};

/// Render the `extern "C"` glue for one interface.
///
/// # Errors
///
/// Fails for surface the sync JVM backend does not implement (async
/// functions, data enums, objects) and for types that cannot mirror into
/// `#[repr(C)]` structs (unresolved or recursive records).
pub fn render(interface: &ir::Interface) -> Result<RenderedJvm, RenderError> {
    let model = Model::new(interface)?;
    let user = names::rust_ident(&interface.name)?;
    let glue_ident = format_ident!("__unibind_jvm_{}", interface.name.trim_start_matches('_'));

    let runtime = types::runtime();
    let records = types::record_mirrors(interface)?;
    let envelopes = types::envelopes(interface);
    let asserts = asserts::layout_asserts(interface, &model);
    let decode_helpers = decode::helpers();
    let encode_helpers = encode::helpers();
    let panic_helpers = function::helpers();
    let functions = interface
        .functions
        .iter()
        .map(|func| function::render_fn(func, interface, &model, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let abi = function::abi_version(&interface.name);
    let module_doc = format!(
        "unibind JVM glue for `{}`: `extern \"C\"` exports consumed by the generated Java \
         Panama binding.",
        interface.name
    );

    let glue = quote! {
        #[doc = #module_doc]
        #[doc(hidden)]
        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            dead_code,
            missing_docs,
            unsafe_code,
            unused_qualifications
        )]
        mod #glue_ident {
            #runtime
            #records
            #envelopes
            #asserts
            #decode_helpers
            #encode_helpers
            #panic_helpers
            #(#functions)*
            #abi
        }
    };
    Ok(RenderedJvm { glue })
}
