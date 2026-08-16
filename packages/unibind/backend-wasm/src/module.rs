//! Assemble the hidden glue module for one interface.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use unibind_core::ir;
use unibind_core::render::{self, RenderError, RenderedInterface, name_ident};

use crate::ty::TyCtx;
use crate::{convert, error, function, object, record, stream, twin};

/// Render `wasm-bindgen` glue for one interface.
///
/// # Errors
///
/// Fails for surface the wasm backend does not implement yet (integer-keyed
/// maps), for surface it refuses on purpose (`blocking`, which has no thread to
/// free), and for renames that cannot become identifiers. Data-carrying enums
/// never reach here: lowering refuses them.
pub fn render(interface: &ir::Interface) -> Result<RenderedInterface, RenderError> {
    // Every reference to the user's module goes through one alias bound here, at
    // the glue module's own scope. A binding library that relocates the items it
    // expands (napi-derive does, into generated helper modules) turns a
    // `super::` written inside one of them into a hop that lands one level short
    // of the crate root, and nothing can inject items into a generated module to
    // fix it. The ts backend was bitten by exactly that; the alias costs one
    // line and makes the question moot whatever the macro does with the tokens.
    let module = name_ident(&interface.name)?;
    let user = format_ident!("__unibind_user");
    let user_alias = quote! {
        #[allow(unused_imports)]
        use super::#module as #user;
    };
    let ctx = TyCtx {
        user: &user,
        objects: &interface.objects,
        enums: &interface.enums,
    };
    let glue_ident = format_ident!("__unibind_wasm_{}", interface.name.trim_start_matches('_'));

    check_blocking(interface)?;
    for rec in &interface.records {
        record::check_record(rec)?;
    }
    let helpers = convert::helpers(interface, &ctx)?;
    let twins = interface
        .records
        .iter()
        .map(|record| twin::render_twin(record, &ctx))
        .collect::<Result<Vec<_>, _>>()?;
    let conversions = interface
        .errors
        .iter()
        .map(|err| error::render_error(err, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let wrappers = interface
        .functions
        .iter()
        .map(|func| function::render_fn(func, &ctx))
        .collect::<Result<Vec<_>, _>>()?;
    let objects = interface
        .objects
        .iter()
        .map(|obj| object::render_object(obj, &ctx))
        .collect::<Result<Vec<_>, _>>()?;
    // Every stream class renders here rather than next to its export: a method's
    // class cannot live inside the object's own exported impl.
    let streams = render::stream_exports(interface)
        .iter()
        .map(|export| stream::render(export, &ctx))
        .collect::<Result<Vec<_>, _>>()?;
    let signal = needs_signal(interface).then(abort_signal);
    let module_docs = function::doc_attrs(&interface.docs);

    let glue = quote! {
        #module_docs
        #[doc(hidden)]
        #[allow(clippy::all, clippy::pedantic, clippy::nursery, unused_qualifications)]
        mod #glue_ident {
            #user_alias
            #signal
            #helpers
            #(#twins)*
            #(#conversions)*
            #(#wrappers)*
            #(#objects)*
            #(#streams)*
        }
    };
    let records = interface.records.iter().map(record::record_attrs).collect();
    Ok(RenderedInterface { glue, records })
}

/// Refuse `blocking`, naming the idiom it owes.
///
/// `blocking` means "release the GIL while this runs", which is a statement
/// about Python's interpreter lock. The ts backend can render it as a plain
/// sync export because Node has a thread to occupy; wasm does not -- the
/// engine's one thread is the caller's -- so the flag would be a promise the
/// boundary cannot keep, and silently dropping it is how a caller learns about
/// it from a frozen tab.
fn check_blocking(interface: &ir::Interface) -> Result<(), RenderError> {
    let members = interface.objects.iter().flat_map(|object| {
        object
            .constructor
            .iter()
            .chain(object.associated.iter())
            .chain(object.methods.iter())
            .map(move |function| (Some(object.name.as_str()), function))
    });
    let free = interface
        .functions
        .iter()
        .map(|function| (None, function));
    for (owner, function) in free.chain(members) {
        if !function.blocking {
            continue;
        }
        let named = owner.map_or_else(
            || function.name.clone(),
            |object| format!("{object}.{}", function.name),
        );
        return Err(RenderError::new(format!(
            "`{named}` is `blocking`; the idiom it owes is a plain sync \
             function or an `async fn`, because the engine's one thread is the \
             caller's and there is nothing for the wasm boundary to block on -- \
             the wasm backend renders an `async fn` as a `Promise` on the \
             microtask queue, which is what a caller wanting not to be blocked \
             actually needs; drop `blocking`"
        )));
    }
    Ok(())
}

/// Whether anything async renders, which is what pulls in the `AbortSignal`
/// bridge.
fn needs_signal(interface: &ir::Interface) -> bool {
    let fns = interface.functions.iter();
    let members = interface
        .objects
        .iter()
        .flat_map(|object| object.methods.iter().chain(object.associated.iter()));
    fns.chain(members)
        .any(|function| matches!(function.asyncness, ir::Asyncness::Async))
}

/// The bridge from a JavaScript `AbortSignal` onto the tokio side.
///
/// `wasm-bindgen` has no cancellation story of its own -- a `Promise` cannot be
/// cancelled from the JavaScript side at all -- so the glue listens for the
/// signal's `abort` event and wakes a `Notify`;
/// `__unibind_wasm_with_abort` `select!`s on it and dropping the user future is
/// the cancellation. Every async wrapper routes through that one helper rather
/// than repeating the race per binding, and the rejection is the same
/// `__unibind__:aborted` the ts backend sends.
///
/// The signal itself arrives as a `js_sys::Object` and is cast here rather than
/// declared as the imported type: an argument of an imported type would make
/// every async export's ABI depend on the `AbortSignal` binding, and the two
/// members below are all the bridge ever touches.
fn abort_signal() -> TokenStream {
    quote! {
        #[::wasm_bindgen::prelude::wasm_bindgen]
        extern "C" {
            /// The `AbortSignal` surface the bridge uses. `structural` reads
            /// each member off the object it was handed instead of through the
            /// class, so a signal from another realm still works.
            #[wasm_bindgen(js_name = "AbortSignal")]
            pub type __UnibindWasmAbortSignal;

            #[wasm_bindgen(method, getter, structural)]
            fn aborted(this: &__UnibindWasmAbortSignal) -> bool;

            #[wasm_bindgen(method, structural, js_name = "addEventListener")]
            fn add_event_listener(
                this: &__UnibindWasmAbortSignal,
                event: &str,
                listener: &::js_sys::Function,
            );

            #[wasm_bindgen(method, structural, js_name = "removeEventListener")]
            fn remove_event_listener(
                this: &__UnibindWasmAbortSignal,
                event: &str,
                listener: &::js_sys::Function,
            );
        }

        /// The rejection an aborted call settles with, on the same channel as
        /// every other generated error.
        fn __unibind_wasm_aborted() -> ::wasm_bindgen::JsValue {
            __unibind_wasm_error(::std::string::String::from("__unibind__:aborted"))
        }

        /// Race `future` against `signal`; the shared body of every async
        /// export. The `biased` arm keeps an abort that raced completion
        /// deterministic (the abort wins), and dropping the user future is the
        /// cancellation.
        async fn __unibind_wasm_with_abort<__UnibindOutput>(
            signal: ::std::option::Option<::js_sys::Object>,
            future: impl ::std::future::Future<Output = __UnibindOutput>,
        ) -> ::std::result::Result<__UnibindOutput, ::wasm_bindgen::JsValue> {
            let ::std::option::Option::Some(signal) = signal else {
                return ::std::result::Result::Ok(future.await);
            };
            let signal = ::wasm_bindgen::JsCast::unchecked_into::<
                __UnibindWasmAbortSignal,
            >(signal);
            if signal.aborted() {
                return ::std::result::Result::Err(__unibind_wasm_aborted());
            }
            let notify = ::std::sync::Arc::new(::tokio::sync::Notify::new());
            let notifier = ::std::sync::Arc::clone(&notify);
            let listener = ::wasm_bindgen::closure::Closure::<dyn ::std::ops::FnMut()>::new(
                move || notifier.notify_one(),
            );
            let callback = ::wasm_bindgen::JsCast::unchecked_ref::<::js_sys::Function>(
                ::std::convert::AsRef::<::wasm_bindgen::JsValue>::as_ref(&listener),
            );
            signal.add_event_listener("abort", callback);
            let settled = ::tokio::select! {
                biased;
                () = notify.notified() => {
                    ::std::result::Result::Err(__unibind_wasm_aborted())
                }
                value = future => ::std::result::Result::Ok(value),
            };
            // Unregister before `listener` drops at the end of this scope. One
            // signal aborts several calls, so the registration outlives this
            // one, and a dropped closure invoked from JavaScript throws.
            signal.remove_event_listener("abort", callback);
            settled
        }
    }
}
