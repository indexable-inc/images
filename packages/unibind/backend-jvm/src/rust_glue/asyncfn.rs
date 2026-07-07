//! Render async `extern "C"` exports: the spawning export plus its
//! `__cancel` / `__task_free` companions.
//!
//! Arguments decode to owned values (lowering guarantees async signatures
//! own their arguments) before anything is spawned, so Java may release
//! the argument arena the moment the downcall returns. The callback fires
//! exactly once per call: from the spawned task's finish closure, or
//! synchronously when argument decoding panics.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::model::Model;
use crate::rust_glue::envelope::{self, EnvelopeParts};
use crate::rust_glue::{function, types, Export};
use crate::{names, RenderError};

/// One async export and its task companions.
pub(crate) fn render_async(
    export: &Export<'_>,
    interface: &ir::Interface,
    model: &Model<'_>,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    let base = export.symbol(interface);
    let export_ident = format_ident!("{base}");
    let envelope = types::envelope_ident(export.owner, &export.function.name);
    let plan = function::arg_plan(export.function, model, user)?;

    let mut params = Vec::new();
    let mut receiver_setup = None;
    if export.has_receiver {
        let owner_ident = names::rust_ident(export.owner.expect("methods have an owner"))?;
        params.push(quote!(this: *mut ::core::ffi::c_void));
        // The task owns a strong count of its own: Java may free the
        // object handle while the call is still in flight.
        receiver_setup = Some(quote! {
            let this = this.cast_const().cast::<super::#user::#owner_ident>();
            unsafe { ::std::sync::Arc::increment_strong_count(this) };
            let this = unsafe { ::std::sync::Arc::from_raw(this) };
        });
    }
    params.extend(plan.params.iter().cloned());

    let path = function::call_path(export, user)?;
    let idents = &plan.idents;
    let call = if export.has_receiver {
        quote!(#path(&this #(, #idents)*).await)
    } else {
        quote!(#path(#(#idents),*).await)
    };

    let parts = EnvelopeParts {
        interface,
        model,
        user,
        function: export.function,
        ret: export.ret.as_ref(),
        envelope: &envelope,
    };
    let completed = envelope::envelope_expr(&parts, &quote!(value))?;
    let zero_value = envelope::zero_value(&parts);
    let bindings = &plan.bindings;
    let docs = &export.function.docs;
    let free = free_companions(&base, &envelope);

    Ok(quote! {
        #(#[doc = #docs])*
        #[doc = ""]
        #[doc = "Asynchronous: returns an opaque task handle and fires `cb` exactly once with a boxed envelope (`code` -2 when cancelled). When argument decoding panics, `cb` fires synchronously with `code` -1 and the returned handle is an inert task whose finish closure does nothing, so `__cancel`/`__task_free` stay uniform and `cb` never fires twice."]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #export_ident(
            #(#params,)*
            cb: unsafe extern "C" fn(*mut ::core::ffi::c_void, *mut #envelope),
            user_data: *mut ::core::ffi::c_void,
        ) -> *mut ::core::ffi::c_void {
            let user_data = SendPtr(user_data);
            let decoded = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                #(#bindings)*
                (#(#idents,)*)
            }));
            match decoded {
                ::std::result::Result::Ok((#(#idents,)*)) => {
                    #receiver_setup
                    ::unibind_runtime::jvm::spawn_cancellable(
                        async move { #call },
                        move |outcome| {
                            let envelope = match outcome {
                                ::unibind_runtime::jvm::TaskOutcome::Completed(value) => #completed,
                                ::unibind_runtime::jvm::TaskOutcome::Panicked(text) => #envelope {
                                    code: -1,
                                    err_msg: string_value(text),
                                    #zero_value
                                },
                                ::unibind_runtime::jvm::TaskOutcome::Cancelled => #envelope {
                                    code: -2,
                                    err_msg: null_string(),
                                    #zero_value
                                },
                            };
                            unsafe {
                                cb(user_data.0, ::std::boxed::Box::into_raw(::std::boxed::Box::new(envelope)))
                            }
                        },
                    )
                }
                ::std::result::Result::Err(payload) => {
                    let envelope = #envelope {
                        code: -1,
                        err_msg: string_value(panic_text(payload.as_ref())),
                        #zero_value
                    };
                    unsafe {
                        cb(user_data.0, ::std::boxed::Box::into_raw(::std::boxed::Box::new(envelope)))
                    };
                    ::unibind_runtime::jvm::spawn_cancellable(
                        async {},
                        |_outcome: ::unibind_runtime::jvm::TaskOutcome<()>| {},
                    )
                }
            }
        }

        #free
    })
}

fn free_companions(base: &str, envelope: &Ident) -> TokenStream {
    let free_ident = format_ident!("{}", names::free_suffix(base));
    let cancel_ident = format_ident!("{}", names::cancel_suffix(base));
    let task_free_ident = format_ident!("{}", names::task_free_suffix(base));
    quote! {
        /// Reclaim an envelope delivered to the paired export's callback.
        /// Null is a no-op; anything else must come from that callback,
        /// exactly once.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #free_ident(envelope: *mut #envelope) {
            if envelope.is_null() {
                return;
            }
            drop(unsafe { ::std::boxed::Box::from_raw(envelope) });
        }

        /// Request cancellation of a task returned by the paired export.
        /// Idempotent and safe after completion; never call it after
        /// `__task_free`. Does not free the handle.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #cancel_ident(task: *mut ::core::ffi::c_void) {
            unsafe { ::unibind_runtime::jvm::task_cancel(task) }
        }

        /// Release a task handle returned by the paired export, exactly
        /// once, after its callback fired.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #task_free_ident(task: *mut ::core::ffi::c_void) {
            unsafe { ::unibind_runtime::jvm::task_free(task) }
        }
    }
}
