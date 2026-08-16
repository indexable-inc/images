//! Render the handle class behind a `#[unibind::object]` type.
//!
//! The user's struct never crosses; JavaScript holds a generated class
//! wrapping an `Arc<T>`. Async methods clone the `Arc` before spawning their
//! future, because the future outlives the borrow of `&self` (see
//! [`crate::function`]); the optional `#[unibind(constructor)]` renders as the
//! `wasm-bindgen` constructor, and `object(resource)` adds an idempotent
//! generated `close()` over the user's own close method.
//!
//! No `Drop` leak warning, unlike the ts and Python backends. A wasm handle is
//! freed when JavaScript calls the generated `free()` (or drops the last
//! reference the bindings hold), and neither engine promises that ever runs, so
//! a `Drop` that warns would fire on some exits and not others. Detecting a
//! leaked resource belongs to the JavaScript wrapper, where a
//! `FinalizationRegistry` can say it without a false negative; that is a
//! follow-up, not a silent gap.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

use crate::function::{Call, Callee, doc_attrs, render_callable, wrapper_parts};
use crate::ty::{self, TyCtx};
use crate::{error, names};

pub fn render_object(object: &ir::Object, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let name = name_ident(&object.name)?;
    let handle = ty::object_handle_ident(object);
    let js_name = names::js_type(&object.names, &object.name);

    // Resources track closedness so close() is idempotent; plain objects carry
    // no extra state.
    let closed_field = object.resource.then(|| {
        quote! { closed: ::std::sync::atomic::AtomicBool, }
    });
    let closed_init = object.resource.then(|| {
        quote! { closed: ::std::sync::atomic::AtomicBool::new(false), }
    });

    let constructor = object
        .constructor
        .as_ref()
        .map(|ctor| render_constructor(ctor, object, ctx))
        .transpose()?;

    let mut associated = Vec::new();
    for function in &object.associated {
        associated.push(render_associated(function, object, ctx)?);
    }

    let mut methods = Vec::new();
    for method in &object.methods {
        // The resource surface owns `close`: the generic path would render a
        // second, non-idempotent close.
        if object.resource && is_close(method) {
            continue;
        }
        methods.push(render_method(method, &object.name, ctx)?);
    }
    let resource_surface = object
        .resource
        .then(|| resource_surface(object))
        .transpose()?;

    let docs = doc_attrs(&object.docs);
    Ok(quote! {
        #docs
        #[::wasm_bindgen::prelude::wasm_bindgen(js_name = #js_name)]
        pub struct #handle {
            inner: ::std::sync::Arc<#user::#name>,
            #closed_field
        }

        impl #handle {
            fn __unibind_from(value: #user::#name) -> Self {
                Self {
                    inner: ::std::sync::Arc::new(value),
                    #closed_init
                }
            }
        }

        #[::wasm_bindgen::prelude::wasm_bindgen(js_class = #js_name)]
        impl #handle {
            #constructor
            #(#associated)*
            #(#methods)*
            #resource_surface
        }
    })
}

/// One exported method delegating to the user's `&self` method. Sync bodies
/// call through the handle's `Arc` directly; async bodies clone it first, into
/// the local the `'static` future then owns. A stream return crosses as the
/// owner-scoped handle class [`crate::stream`] renders alongside the object.
fn render_method(
    method: &ir::Function,
    object: &str,
    ctx: &TyCtx<'_>,
) -> Result<TokenStream, RenderError> {
    let method_name = name_ident(&method.name)?;
    let wrapper = wrapper_parts(method, ctx)?;
    let exprs = &wrapper.exprs;
    let call = match method.asyncness {
        ir::Asyncness::Sync => Call {
            prelude: TokenStream::new(),
            expr: quote!(self.inner.#method_name(#(#exprs),*)),
        },
        ir::Asyncness::Async => Call {
            prelude: quote!(let __unibind_inner = ::std::sync::Arc::clone(&self.inner);),
            expr: quote!(__unibind_inner.#method_name(#(#exprs),*)),
        },
    };
    render_callable(method, ctx, &wrapper, &call, Callee::Method { object })
}

/// A `#[unibind(associated)]` function, rendered as a static method.
///
/// The shared callable path does the work, including the error mapping, the
/// `Result` shape, and wrapping an object return into its handle; only the call
/// target differs from a method's, since there is no instance to call through.
/// One returning the object needs no marker of its own: `wasm-bindgen` makes
/// any receiver-less function in the impl a static, and a static handing back
/// the class is exactly the factory JavaScript wants.
fn render_associated(
    function: &ir::Function,
    object: &ir::Object,
    ctx: &TyCtx<'_>,
) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let object_ident = name_ident(&object.name)?;
    let function_name = name_ident(&function.name)?;
    let wrapper = wrapper_parts(function, ctx)?;
    let exprs = &wrapper.exprs;
    // No receiver to clone into the future: an associated function owns its
    // arguments and the object does not exist yet.
    let call = Call {
        prelude: TokenStream::new(),
        expr: quote!(#user::#object_ident::#function_name(#(#exprs),*)),
    };
    render_callable(
        function,
        ctx,
        &wrapper,
        &call,
        Callee::Associated {
            object: &object.name,
        },
    )
}

/// The `wasm-bindgen` constructor over the user's `#[unibind(constructor)]`
/// function. Constructors are sync with an implied return (lowering enforces
/// both), so the shared callable path does not fit.
fn render_constructor(
    ctor: &ir::Function,
    object: &ir::Object,
    ctx: &TyCtx<'_>,
) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let handle = ty::object_handle_ident(object);
    let object_ident = name_ident(&object.name)?;
    let ctor_name = name_ident(&ctor.name)?;
    let docs = doc_attrs(&ctor.docs);
    let wrapper = wrapper_parts(ctor, ctx)?;
    let params = &wrapper.params;
    let prologue = &wrapper.prologue;
    let exprs = &wrapper.exprs;
    let call = quote!(#user::#object_ident::#ctor_name(#(#exprs),*));
    let body = ctor.throws.as_deref().map_or_else(
        || quote!(::std::result::Result::Ok(#handle::__unibind_from(#call))),
        |throws| {
            let throws = error::conversion_ident(throws);
            quote! {
                match #call {
                    ::std::result::Result::Ok(value) => {
                        ::std::result::Result::Ok(#handle::__unibind_from(value))
                    }
                    ::std::result::Result::Err(error) => {
                        ::std::result::Result::Err(#throws(error))
                    }
                }
            }
        },
    );
    Ok(quote! {
        #docs
        #[wasm_bindgen(constructor)]
        pub fn #ctor_name(
            #(#params),*
        ) -> ::std::result::Result<#handle, ::wasm_bindgen::JsValue> {
            #(#prologue)*
            #body
        }
    })
}

/// The generated idempotent `close()`: `swap` picks one winner between racing
/// calls, so the user's close runs at most once and a second call resolves to a
/// no-op. An async close decides the winner before the future is spawned, which
/// still admits exactly one.
fn resource_surface(object: &ir::Object) -> Result<TokenStream, RenderError> {
    let close = object
        .methods
        .iter()
        .find(|method| is_close(method))
        .ok_or_else(|| {
            RenderError::new(format!(
                "`{}` is a resource without a close method; lowering guarantees one",
                object.name
            ))
        })?;
    let docs = doc_attrs(&close.docs);
    Ok(match close.asyncness {
        ir::Asyncness::Sync => {
            let stmt = close.throws.as_deref().map_or_else(
                || quote!(self.inner.close();),
                |throws| {
                    let throws = error::conversion_ident(throws);
                    quote! {
                        if let ::std::result::Result::Err(error) = self.inner.close() {
                            return ::std::result::Result::Err(#throws(error));
                        }
                    }
                },
            );
            quote! {
                #docs
                #[wasm_bindgen(js_name = "close")]
                pub fn close(&self) -> ::std::result::Result<(), ::wasm_bindgen::JsValue> {
                    if self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst) {
                        return ::std::result::Result::Ok(());
                    }
                    #stmt
                    ::std::result::Result::Ok(())
                }
            }
        }
        ir::Asyncness::Async => {
            let stmt = close.throws.as_deref().map_or_else(
                || quote!(__unibind_inner.close().await;),
                |throws| {
                    let throws = error::conversion_ident(throws);
                    quote! {
                        if let ::std::result::Result::Err(error) =
                            __unibind_inner.close().await
                        {
                            return ::std::result::Result::Err(#throws(error));
                        }
                    }
                },
            );
            quote! {
                #docs
                #[wasm_bindgen(js_name = "close")]
                pub fn close(&self) -> ::js_sys::Promise {
                    let __unibind_first =
                        !self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst);
                    let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
                    ::wasm_bindgen_futures::future_to_promise(async move {
                        if __unibind_first {
                            #stmt
                        }
                        ::std::result::Result::Ok(::wasm_bindgen::JsValue::UNDEFINED)
                    })
                }
            }
        }
    })
}

/// The resource teardown shape lowering guarantees: named `close`, zero
/// arguments, no success value (`Result<(), E>` and async both count).
fn is_close(method: &ir::Function) -> bool {
    method.name == "close" && method.args.is_empty() && method.ret.is_none()
}
