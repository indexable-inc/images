//! Assemble the hidden glue module for one interface.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::ty::TyCtx;
use crate::{error, function, object, record, stream, RenderError, RenderedInterface};

/// Render `napi-rs` glue for one interface.
///
/// # Errors
///
/// Fails for surface the ts backend does not implement yet (data enums,
/// BigInt-only integers, integer-keyed maps, stream-returning methods) and
/// for renames that cannot become identifiers.
pub fn render(interface: &ir::Interface) -> Result<RenderedInterface, RenderError> {
    if let Some(data_enum) = interface.enums.first() {
        return Err(RenderError::new(format!(
            "`{}` is a data enum, which the ts backend does not render",
            data_enum.name
        )));
    }

    let user = crate::ty::name_ident(&interface.name)?;
    let ctx = TyCtx {
        user: &user,
        objects: &interface.objects,
    };
    let glue_ident = format_ident!("__unibind_ts_{}", interface.name.trim_start_matches('_'));

    for rec in &interface.records {
        record::check_record(rec)?;
    }
    let conversions = interface
        .errors
        .iter()
        .map(|err| error::render_error(err, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let wrappers = interface
        .functions
        .iter()
        .map(|func| match &func.ret {
            Some(ir::Type::Stream(element)) => stream::render_stream_fn(func, element, &ctx),
            _ => function::render_fn(func, &ctx),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let objects = interface
        .objects
        .iter()
        .map(|obj| object::render_object(obj, &ctx))
        .collect::<Result<Vec<_>, _>>()?;
    let signal = needs_signal(interface).then(abort_signal);
    let module_docs = function::doc_attrs(&interface.docs);

    let glue = quote! {
        #module_docs
        #[doc(hidden)]
        #[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
        mod #glue_ident {
            #signal
            #(#conversions)*
            #(#wrappers)*
            #(#objects)*
        }
    };
    let records = interface.records.iter().map(record::record_attrs).collect();
    Ok(RenderedInterface { glue, records })
}

/// Whether anything async renders, which is what pulls in the
/// `AbortSignal` bridge.
fn needs_signal(interface: &ir::Interface) -> bool {
    let fns = interface.functions.iter();
    let methods = interface.objects.iter().flat_map(|object| object.methods.iter());
    fns.chain(methods)
        .any(|function| matches!(function.asyncness, ir::Asyncness::Async))
}

/// The bridge from a JavaScript `AbortSignal` onto the tokio side. napi's
/// own `AbortSignal` type only cancels `AsyncTask` work queue entries, so
/// the glue registers an `on_abort` callback that wakes a `Notify`; the
/// wrapper `select!`s on it and dropping the user future is the
/// cancellation.
fn abort_signal() -> TokenStream {
    quote! {
        /// One trailing optional argument on every async export; `undefined`
        /// (or omission) crosses as `None`.
        pub struct __UnibindAbortSignal {
            already_aborted: bool,
            notify: ::std::sync::Arc<::tokio::sync::Notify>,
        }

        impl ::napi::bindgen_prelude::FromNapiValue for __UnibindAbortSignal {
            unsafe fn from_napi_value(
                env: ::napi::sys::napi_env,
                value: ::napi::sys::napi_value,
            ) -> ::napi::Result<Self> {
                let object = unsafe {
                    <::napi::bindgen_prelude::Object as ::napi::bindgen_prelude::FromNapiValue>::from_napi_value(env, value)?
                };
                let already_aborted = object.get::<bool>("aborted")?.unwrap_or(false);
                let signal = unsafe {
                    <::napi::bindgen_prelude::AbortSignal as ::napi::bindgen_prelude::FromNapiValue>::from_napi_value(env, value)?
                };
                let notify = ::std::sync::Arc::new(::tokio::sync::Notify::new());
                let notifier = ::std::sync::Arc::clone(&notify);
                signal.on_abort(move || notifier.notify_one());
                ::std::result::Result::Ok(Self {
                    already_aborted,
                    notify,
                })
            }
        }

        fn __unibind_aborted() -> ::napi::Error {
            ::napi::Error::new(::napi::Status::Cancelled, "__unibind__:aborted")
        }
    }
}
