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
    // the same lint. `dead_code` is allowed because every annotated record
    // gets a mirror even when no export mentions it, and the mirrors live in
    // this private module.
    let glue = quote! {
        #[doc(hidden)]
        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            dead_code,
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
pub fn report_module(interface: &ir::Interface) -> String {
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

/// Refuse the IR surface the Rust backend does not render yet, plus the
/// names its generated code claims for itself. Objects and data enums are
/// whole shapes; streams are supported only as a sync function's plain
/// return (`fn f(..) -> UniStream<T>`), because the `async` and `Result`
/// compositions need dedicated wrapper designs.
pub fn reject_unrendered(interface: &ir::Interface) -> Result<(), RenderError> {
    reject_raw_names(interface)?;
    reject_reserved_names(interface)?;
    reject_wrapper_collisions(interface)?;
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

/// Refuse raw identifiers (`r#type`, `r#match`, ...) anywhere on the
/// boundary: the backend derives exported symbol strings, carrier names,
/// and wrapper names from these names, and a `r#` prefix survives none of
/// those derivations.
fn reject_raw_names(interface: &ir::Interface) -> Result<(), RenderError> {
    let function_names = interface.functions.iter().flat_map(|func| {
        std::iter::once(&func.name).chain(func.args.iter().map(|arg| &arg.name))
    });
    let record_names = interface.records.iter().flat_map(|rec| {
        std::iter::once(&rec.name).chain(rec.fields.iter().map(|field| &field.name))
    });
    let error_names = interface.errors.iter().flat_map(|err| {
        std::iter::once(&err.name).chain(err.variants.iter().map(|var| &var.name))
    });
    for name in function_names.chain(record_names).chain(error_names) {
        if name.starts_with("r#") {
            return Err(RenderError::new(format!(
                "`{name}` is a raw identifier; the Rust backend derives symbol \
                 and type names from boundary names, so raw identifiers cannot \
                 cross — rename it"
            )));
        }
    }
    Ok(())
}

/// Refuse the names the generated glue and client claim for themselves:
/// the handshake export, `Engine::load`, the `Engine`/`LoadError` support
/// types, and the `<Error>Stable` carrier structs.
fn reject_reserved_names(interface: &ir::Interface) -> Result<(), RenderError> {
    for function in &interface.functions {
        if function.name == "ir_sha256" {
            return Err(RenderError::new(
                "`ir_sha256` is reserved: the Rust backend exports the IR-hash \
                 handshake as `unibind_<module>_ir_sha256`; rename the function",
            ));
        }
        if function.name == "load" {
            return Err(RenderError::new(
                "`load` is reserved: the generated Rust client already has \
                 `Engine::load`; rename the function",
            ));
        }
    }
    for name in interface
        .records
        .iter()
        .map(|rec| &rec.name)
        .chain(interface.errors.iter().map(|err| &err.name))
    {
        if name == "Engine" || name == "LoadError" {
            return Err(RenderError::new(format!(
                "`{name}` is reserved: the generated Rust client already \
                 exports a support type with that name; rename it"
            )));
        }
    }
    for error in &interface.errors {
        let carrier = format!("{}Stable", error.name);
        if interface.records.iter().any(|rec| rec.name == carrier) {
            return Err(RenderError::new(format!(
                "record `{carrier}` collides with the ABI carrier the Rust \
                 backend generates for error `{}`; rename one of them",
                error.name
            )));
        }
    }
    Ok(())
}

/// Refuse function pairs whose generated future/stream wrapper names
/// collide: pascal-casing collapses consecutive underscores, so `foo_bar`
/// and `foo__bar` would both produce `FooBarFuture`.
fn reject_wrapper_collisions(interface: &ir::Interface) -> Result<(), RenderError> {
    let mut seen: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
    for func in &interface.functions {
        let wrapper = if matches!(func.asyncness, ir::Asyncness::Async) {
            function::future_wrapper_ident(func).to_string()
        } else if function::returns_stream(func) {
            function::stream_wrapper_ident(func).to_string()
        } else {
            continue;
        };
        if let Some(previous) = seen.insert(wrapper.clone(), func.name.as_str()) {
            return Err(RenderError::new(format!(
                "`{previous}` and `{}` both generate a wrapper named \
                 `{wrapper}`; rename one so the pascal-cased names differ",
                func.name
            )));
        }
    }
    Ok(())
}
