//! Assemble the engine glue module and the IR-hash handshake.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use sha2::{Digest as _, Sha256};
use unibind_core::ir;

use crate::ty::{self, Paths};
use crate::{error, function, record, RenderError, RenderedInterface};

/// Render the engine glue for one interface: a hidden sibling module with
/// the record mirrors, error carriers, one `#[stabby::export]` wrapper per
/// function, and the handshake symbol.
///
/// # Errors
///
/// Fails for surface this backend does not implement yet: data enums,
/// objects, and the stream compositions (`async` / `Result`) listed on
/// [`reject_unrendered`].
pub fn render(interface: &ir::Interface) -> Result<RenderedInterface, RenderError> {
    reject_unrendered(interface)?;
    let user = ty::name_ident(&interface.name);
    let glue_ident = format_ident!("__unibind_rs_{}", interface.name.trim_start_matches('_'));
    let paths = Paths {
        plain: quote!(super::#user::),
        mirror: TokenStream::new(),
        report_module: report_module(interface),
    };

    let mirrors = interface.records.iter().map(|rec| {
        let mirror = record::mirror_struct(rec, &paths);
        let conversions = record::mirror_conversions(rec, &paths);
        quote! {
            #mirror
            #conversions
        }
    });
    let errors = interface.errors.iter().map(|err| {
        let stable = error::stable_struct(err, &paths);
        let conversion = error::engine_conversion(err, &paths);
        quote! {
            #stable
            #conversion
        }
    });
    let exports = interface
        .functions
        .iter()
        .map(|func| function::render_export(interface, func, &paths));
    let handshake = handshake(interface)?;

    // The wrapper bodies are macro-generated (hence lint-exempt like the
    // Python glue); `improper_ctypes_definitions` is allowed because stabby's
    // repr(C) types are FFI-sound by construction and its own macros silence
    // the same lint.
    let glue = quote! {
        #[doc(hidden)]
        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            unused_qualifications,
            improper_ctypes_definitions
        )]
        mod #glue_ident {
            #(#mirrors)*
            #(#errors)*
            #(#exports)*
            #handshake
        }
    };
    Ok(RenderedInterface { glue })
}

/// The logical namespace stamped into every generated mirror's stabby
/// report (`module = ...`), shared by engine and client so the structural
/// check compares like against like.
pub(crate) fn report_module(interface: &ir::Interface) -> String {
    format!("unibind::{}", interface.name)
}

/// The hex SHA-256 of the interface's serialized IR: hashed over the exact
/// bytes `unibind_core::embed` plants in the link section, so the engine's
/// handshake symbol and a client generated from the same source cannot
/// disagree unless the interfaces differ.
///
/// # Errors
///
/// Fails only when the interface cannot serialize (a bug in the IR types).
pub fn ir_sha256_hex(interface: &ir::Interface) -> Result<String, RenderError> {
    let json = unibind_core::embed::ir_json(interface)
        .map_err(|lower_error| RenderError::new(lower_error.message))?;
    Ok(hex::encode(Sha256::digest(&json)))
}

fn handshake(interface: &ir::Interface) -> Result<TokenStream, RenderError> {
    let symbol = ty::name_ident(&function::ir_sha256_symbol(interface));
    let hash = ir_sha256_hex(interface)?;
    Ok(quote! {
        #[::stabby::export]
        pub extern "C" fn #symbol() -> ::stabby::str::Str<'static> {
            ::stabby::str::Str::from(#hash)
        }
    })
}

/// Refuse the IR surface the Rust backend does not render yet. Objects and
/// data enums are whole shapes; streams are supported only as a sync
/// function's plain return (`fn f(..) -> UniStream<T>`), because the
/// `async` and `Result` compositions need dedicated wrapper designs.
pub(crate) fn reject_unrendered(interface: &ir::Interface) -> Result<(), RenderError> {
    if let Some(object) = interface.objects.first() {
        return Err(RenderError::new(format!(
            "`{}` is a #[unibind::object]; the Rust backend does not render \
             objects yet — restrict the module with backends(py) or split the \
             boundary",
            object.name
        )));
    }
    if let Some(data_enum) = interface.enums.first() {
        return Err(RenderError::new(format!(
            "`{}` is a data enum, which the Rust backend does not render yet",
            data_enum.name
        )));
    }
    for function in &interface.functions {
        if !matches!(function.ret, Some(ir::Type::Stream(_))) {
            continue;
        }
        if matches!(function.asyncness, ir::Asyncness::Async) {
            return Err(RenderError::new(format!(
                "`{}` is an async fn returning a stream; the Rust backend \
                 renders streams from sync fns only",
                function.name
            )));
        }
        if function.throws.is_some() {
            return Err(RenderError::new(format!(
                "`{}` returns Result<UniStream<_>, _>; the Rust backend \
                 renders infallible stream returns only",
                function.name
            )));
        }
    }
    Ok(())
}
