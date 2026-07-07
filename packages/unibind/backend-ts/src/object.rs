//! Render the handle class behind a `#[unibind::object]` type.
//!
//! The user's struct never crosses; JavaScript holds a generated class
//! wrapping an `Arc<T>`. Methods clone the `Arc` out for async bodies (so
//! the receiver outlives the call), the optional `#[unibind(constructor)]`
//! renders as the napi constructor, and `object(resource)` adds an
//! idempotent generated `close()` over the user's own close method plus an
//! unclosed-leak warning on drop, mirroring the Python backend's
//! `ResourceWarning`.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;

use crate::function::{doc_attrs, render_callable, wrapper_parts};
use crate::ty::{self, TyCtx};
use crate::RenderError;

pub fn render_object(object: &ir::Object, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let name = ty::name_ident(&object.name)?;
    let handle = ty::object_handle_ident(object);
    let js_name = object.names.ts.clone().unwrap_or_else(|| object.name.clone());

    // Resources track closedness so close() is idempotent and Drop can
    // warn about leaks; plain objects carry no extra state.
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

    let mut methods = Vec::new();
    for method in &object.methods {
        if matches!(method.ret, Some(ir::Type::Stream(_))) {
            return Err(RenderError::new(format!(
                "method `{}` of `{}` returns a stream; stream methods are a \
                 follow-up of issue #1993 (return the stream from a module \
                 function instead)",
                method.name, object.name
            )));
        }
        // The resource surface owns `close`: the generic path would render
        // a second, non-idempotent close.
        if object.resource && is_close(method) {
            continue;
        }
        methods.push(render_method(method, ctx)?);
    }
    let resource_surface = object
        .resource
        .then(|| resource_surface(object))
        .transpose()?;
    let leak_warning = object.resource.then(|| leak_warning(&handle, &js_name));

    let docs = doc_attrs(&object.docs);
    Ok(quote! {
        #docs
        #[::napi_derive::napi(js_name = #js_name)]
        pub struct #handle {
            inner: ::std::sync::Arc<super::#user::#name>,
            #closed_field
        }

        impl #handle {
            fn __unibind_from(value: super::#user::#name) -> Self {
                Self {
                    inner: ::std::sync::Arc::new(value),
                    #closed_init
                }
            }
        }

        #[::napi_derive::napi]
        impl #handle {
            #constructor
            #(#methods)*
            #resource_surface
        }

        #leak_warning
    })
}

/// One `#[napi]` method delegating to the user's `&self` method. Sync
/// bodies call through the handle's `Arc` directly; async bodies clone the
/// `Arc` into the future so the receiver outlives a collected handle.
fn render_method(method: &ir::Function, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let method_name = ty::name_ident(&method.name)?;
    let wrapper = wrapper_parts(method, ctx)?;
    let exprs = &wrapper.exprs;
    let call = match method.asyncness {
        ir::Asyncness::Sync => quote!(self.inner.#method_name(#(#exprs),*)),
        ir::Asyncness::Async => quote! {
            {
                let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
                async move { __unibind_inner.#method_name(#(#exprs),*).await }
            }
        },
    };
    render_callable(method, ctx, &wrapper, &call, Some(&quote!(&self,)))
}

/// The napi constructor over the user's `#[unibind(constructor)]` function.
/// Constructors are sync with an implied return (lowering enforces both),
/// so the shared callable path does not fit.
fn render_constructor(
    ctor: &ir::Function,
    object: &ir::Object,
    ctx: &TyCtx<'_>,
) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let object_ident = ty::name_ident(&object.name)?;
    let ctor_name = ty::name_ident(&ctor.name)?;
    let docs = doc_attrs(&ctor.docs);
    let wrapper = wrapper_parts(ctor, ctx)?;
    let params = &wrapper.params;
    let exprs = &wrapper.exprs;
    let call = quote!(super::#user::#object_ident::#ctor_name(#(#exprs),*));
    let body = if ctor.throws.is_some() {
        quote! {
            match #call {
                ::std::result::Result::Ok(value) => {
                    ::std::result::Result::Ok(Self::__unibind_from(value))
                }
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(::napi::Error::from(error))
                }
            }
        }
    } else {
        quote!(::std::result::Result::Ok(Self::__unibind_from(#call)))
    };
    Ok(quote! {
        #docs
        #[::napi_derive::napi(constructor)]
        pub fn #ctor_name(#(#params),*) -> ::napi::Result<Self> {
            #body
        }
    })
}

/// The generated idempotent `close()`: `swap` picks one winner between
/// racing calls, so the user's close runs at most once and a second call
/// resolves to a no-op. Async user closes decide the winner at first poll,
/// which still admits exactly one.
fn resource_surface(object: &ir::Object) -> Result<TokenStream, RenderError> {
    let close = object.methods.iter().find(|method| is_close(method)).ok_or_else(|| {
        RenderError::new(format!(
            "`{}` is a resource without a close method; lowering guarantees one",
            object.name
        ))
    })?;
    let docs = doc_attrs(&close.docs);
    Ok(match close.asyncness {
        ir::Asyncness::Sync => {
            let stmt = if close.throws.is_some() {
                quote! {
                    if let ::std::result::Result::Err(error) = self.inner.close() {
                        return ::std::result::Result::Err(::napi::Error::from(error));
                    }
                }
            } else {
                quote!(self.inner.close();)
            };
            quote! {
                #docs
                #[::napi_derive::napi]
                pub fn close(&self) -> ::napi::Result<()> {
                    if self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst) {
                        return ::std::result::Result::Ok(());
                    }
                    #stmt
                    ::std::result::Result::Ok(())
                }
            }
        }
        ir::Asyncness::Async => {
            let stmt = if close.throws.is_some() {
                quote! {
                    if let ::std::result::Result::Err(error) = __unibind_inner.close().await {
                        return ::std::result::Result::Err(::napi::Error::from(error));
                    }
                }
            } else {
                quote!(__unibind_inner.close().await;)
            };
            quote! {
                #docs
                #[::napi_derive::napi]
                pub async fn close(&self) -> ::napi::Result<()> {
                    let __unibind_first =
                        !self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst);
                    let __unibind_inner = ::std::sync::Arc::clone(&self.inner);
                    if __unibind_first {
                        #stmt
                    }
                    ::std::result::Result::Ok(())
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

/// Mirror the Python backend's `ResourceWarning`: dropping an unclosed
/// resource warns on stderr, Node's own convention for leaked handles.
fn leak_warning(handle: &proc_macro2::Ident, js_name: &str) -> TokenStream {
    let message = format!("unclosed {js_name}: call close() or use `await using`");
    quote! {
        impl ::std::ops::Drop for #handle {
            fn drop(&mut self) {
                if !self.closed.load(::std::sync::atomic::Ordering::SeqCst) {
                    ::std::eprintln!(#message);
                }
            }
        }
    }
}
