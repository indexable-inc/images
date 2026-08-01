//! Assemble the hidden glue module for one interface.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use unibind_core::ir;
use unibind_core::render::{RenderError, RenderedInterface, name_ident};

use crate::ty::TyCtx;
use crate::{convert, error, function, mirror, object, record, stream};

/// Render `napi-rs` glue for one interface.
///
/// # Errors
///
/// Fails for surface the ts backend does not implement yet (data enums,
/// integer-keyed maps, stream-returning methods) and for renames that
/// cannot become identifiers.
pub fn render(interface: &ir::Interface) -> Result<RenderedInterface, RenderError> {
    if let Some(data_enum) = interface.enums.first() {
        return Err(RenderError::new(format!(
            "`{}` is a data enum, which the ts backend does not render",
            data_enum.name
        )));
    }

    let user = name_ident(&interface.name)?;
    let mirrored = mirror::mirrored_records(&interface.records);
    let ctx = TyCtx {
        user: &user,
        objects: &interface.objects,
        mirrored: &mirrored,
    };
    let glue_ident = format_ident!("__unibind_ts_{}", interface.name.trim_start_matches('_'));

    for rec in &interface.records {
        record::check_record(rec)?;
    }
    let bigint = convert::helpers(interface, &mirrored);
    let mirrors = interface
        .records
        .iter()
        .filter(|record| mirrored.iter().any(|name| *name == record.name))
        .map(|record| mirror::render_mirror(record, &ctx))
        .collect::<Result<Vec<_>, _>>()?;
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
            #bigint
            #(#mirrors)*
            #(#conversions)*
            #(#wrappers)*
            #(#objects)*
        }
    };
    let records = interface
        .records
        .iter()
        .map(|record| record::record_attrs(record, &mirrored))
        .collect();
    Ok(RenderedInterface { glue, records })
}

/// Whether anything async renders, which is what pulls in the
/// `AbortSignal` bridge.
fn needs_signal(interface: &ir::Interface) -> bool {
    let fns = interface.functions.iter();
    let methods = interface
        .objects
        .iter()
        .flat_map(|object| object.methods.iter());
    fns.chain(methods)
        .any(|function| matches!(function.asyncness, ir::Asyncness::Async))
}

/// The bridge from a JavaScript `AbortSignal` onto the tokio side. napi's
/// own `AbortSignal` type only cancels `AsyncTask` work queue entries, so
/// the glue registers an `on_abort` callback that wakes a `Notify`;
/// `__unibind_with_abort` `select!`s on it and dropping the user future
/// is the cancellation. Every async wrapper routes through that one
/// helper rather than repeating the race per binding.
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

        /// Race `future` against `signal`; the shared body of every async
        /// export. The `biased` arm keeps an abort that raced completion
        /// deterministic (the abort wins), and dropping the user future is
        /// the cancellation.
        async fn __unibind_with_abort<__UnibindOutput>(
            signal: ::std::option::Option<__UnibindAbortSignal>,
            future: impl ::std::future::Future<Output = __UnibindOutput>,
        ) -> ::napi::Result<__UnibindOutput> {
            match signal {
                ::std::option::Option::Some(signal) => {
                    if signal.already_aborted {
                        return ::std::result::Result::Err(__unibind_aborted());
                    }
                    ::tokio::select! {
                        biased;
                        () = signal.notify.notified() => {
                            ::std::result::Result::Err(__unibind_aborted())
                        }
                        value = future => ::std::result::Result::Ok(value),
                    }
                }
                ::std::option::Option::None => ::std::result::Result::Ok(future.await),
            }
        }
    }
}
