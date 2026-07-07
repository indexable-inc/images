//! Render sync `extern "C"` exports and the plumbing every export kind
//! shares: argument decoding plans and the panic-text helper.
//!
//! Every export wraps its body in `catch_unwind`, so unwinding never
//! crosses the C boundary: a panic becomes an envelope with `code == -1`
//! carrying the payload text.
//!
//! `#[unibind(blocking)]` needs no special sync handling here: the JVM has
//! no GIL to release, so the calling Java thread blocking for the call's
//! duration is the whole blocking contract.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::model::Model;
use crate::rust_glue::envelope::{self, EnvelopeParts};
use crate::rust_glue::{decode, types, Export};
use crate::{names, RenderError};

/// The shared panic-payload helper.
pub fn helpers() -> TokenStream {
    quote! {
        /// Best-effort text from a caught panic payload.
        fn panic_text(payload: &(dyn ::std::any::Any + ::core::marker::Send)) -> ::std::string::String {
            if let ::std::option::Option::Some(text) = payload.downcast_ref::<&str>() {
                return ::std::borrow::ToOwned::to_owned(*text);
            }
            if let ::std::option::Option::Some(text) = payload.downcast_ref::<::std::string::String>() {
                return ::std::clone::Clone::clone(text);
            }
            ::std::borrow::ToOwned::to_owned("panic across the unibind boundary")
        }
    }
}

/// The ABI version probe.
pub fn abi_version(module: &str) -> TokenStream {
    let ident = format_ident!("{}", names::abi_symbol(module));
    quote! {
        /// ABI revision of these exports; the Java binding checks it at
        /// load.
        #[unsafe(no_mangle)]
        pub extern "C" fn #ident() -> u32 {
            0
        }
    }
}

/// How one export's arguments cross: the `extern "C"` parameters, the
/// decode bindings turning them into the user function's values, and the
/// decoded identifiers to forward.
pub(crate) struct ArgPlan {
    pub params: Vec<TokenStream>,
    pub bindings: Vec<TokenStream>,
    pub idents: Vec<Ident>,
}

/// Plan the argument crossing for one export. Scalars pass by value,
/// aggregates by `*const` mirror pointer into Java-owned memory.
pub(crate) fn arg_plan(
    function: &ir::Function,
    model: &Model<'_>,
    user: &Ident,
) -> Result<ArgPlan, RenderError> {
    let mut plan = ArgPlan {
        params: Vec::new(),
        bindings: Vec::new(),
        idents: Vec::new(),
    };
    for arg in &function.args {
        let ident = names::rust_ident(&arg.name)?;
        let cty = model.boundary(&arg.ty);
        let mirror = types::mirror_tokens(&cty);
        if cty.is_scalar() {
            plan.params.push(quote!(#ident: #mirror));
            if matches!(arg.ty, ir::Type::Bool) {
                plan.bindings.push(quote!(let #ident = #ident != 0;));
            }
        } else {
            plan.params.push(quote!(#ident: *const #mirror));
            let decoded = decode::expr(model, &arg.ty, &quote!(#ident), user)?;
            plan.bindings.push(quote! {
                let #ident = unsafe { &*#ident };
                let #ident = #decoded;
            });
        }
        plan.idents.push(ident);
    }
    Ok(plan)
}

/// The `super::<user>::...` call target for one export.
pub(crate) fn call_path(
    export: &Export<'_>,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    let name = names::rust_ident(&export.function.name)?;
    match export.owner {
        Some(owner) => {
            let owner_ident = names::rust_ident(owner)?;
            Ok(quote!(super::#user::#owner_ident::#name))
        }
        None => Ok(quote!(super::#user::#name)),
    }
}

/// One sync export and its `__free` companion. Constructors render here
/// too (lowering keeps them sync): their envelope carries the new object
/// as an opaque handle.
pub(crate) fn render_sync(
    export: &Export<'_>,
    interface: &ir::Interface,
    model: &Model<'_>,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    let base = export.symbol(interface);
    let export_ident = format_ident!("{base}");
    let free_ident = format_ident!("{}", names::free_suffix(&base));
    let envelope = types::envelope_ident(export.owner, &export.function.name);
    let plan = arg_plan(export.function, model, user)?;

    let mut params = Vec::new();
    let mut receiver_binding = None;
    if export.has_receiver {
        let owner_ident = names::rust_ident(export.owner.expect("methods have an owner"))?;
        params.push(quote!(this: *mut ::core::ffi::c_void));
        // The read stays a shared borrow: Java's per-object read lock
        // guarantees the handle outlives every sync downcall.
        receiver_binding = Some(quote! {
            let this = unsafe { &*this.cast_const().cast::<super::#user::#owner_ident>() };
        });
    }
    params.extend(plan.params.iter().cloned());

    let path = call_path(export, user)?;
    let forwarded = &plan.idents;
    let call = if export.has_receiver {
        quote!(#path(this #(, #forwarded)*))
    } else {
        quote!(#path(#(#forwarded),*))
    };

    let parts = EnvelopeParts {
        interface,
        model,
        user,
        function: export.function,
        ret: export.ret.as_ref(),
        envelope: &envelope,
    };
    let body = envelope::envelope_expr(&parts, &call)?;
    let zero_value = envelope::zero_value(&parts);
    let bindings = &plan.bindings;
    let docs = &export.function.docs;
    let blocking_doc = export.function.blocking.then(|| {
        quote! {
            #[doc = ""]
            #[doc = "`#[unibind(blocking)]`: the calling Java thread blocks for the duration; the JVM has no GIL to release, so blocking the caller is the whole contract."]
        }
    });

    Ok(quote! {
        #(#[doc = #docs])*
        #blocking_doc
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #export_ident(#(#params),*) -> *mut #envelope {
            let outcome = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                #receiver_binding
                #(#bindings)*
                #body
            }));
            let envelope = match outcome {
                ::std::result::Result::Ok(envelope) => envelope,
                ::std::result::Result::Err(payload) => #envelope {
                    code: -1,
                    err_msg: string_value(panic_text(payload.as_ref())),
                    #zero_value
                },
            };
            ::std::boxed::Box::into_raw(::std::boxed::Box::new(envelope))
        }

        /// Reclaim an envelope returned by the paired export. Null is a
        /// no-op; anything else must come from that export, exactly once.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #free_ident(envelope: *mut #envelope) {
            if envelope.is_null() {
                return;
            }
            drop(unsafe { ::std::boxed::Box::from_raw(envelope) });
        }
    })
}
