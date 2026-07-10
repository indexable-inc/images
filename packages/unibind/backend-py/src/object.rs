//! Wrapper classes for `#[unibind::object]` types: a `#[pyclass]` holding
//! the user struct behind an `Arc` (async methods and Python's free
//! aliasing both need shared ownership), plus the constructor, methods,
//! and, for resources, the close/async-with surface from
//! [`crate::resource`].

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::ctx::Ctx;
use crate::function::{doc_attrs, render_method};
use crate::{resource, sig, RenderError};

pub fn render_object(object: &ir::Object, ctx: &Ctx<'_>) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let wrapper = crate::ctx::object_wrapper_ident(&object.name);
    let user_ty = Ident::new(&object.name, Span::call_site());
    let py_name = object.names.py.as_deref().unwrap_or(&object.name);
    let docs = doc_attrs(&object.docs);

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
        // The resource surface owns `close`: the generic path would render
        // a second, non-idempotent close.
        if object.resource && method.name == "close" {
            continue;
        }
        methods.push(render_method(method, ctx, &object.name)?);
    }
    let resource_surface = object.resource.then(|| resource::surface(object));
    let leak_warning = object.resource.then(|| resource::leak_warning(object));

    Ok(quote! {
        #docs
        #[::pyo3::pyclass(name = #py_name, frozen)]
        struct #wrapper {
            inner: ::std::sync::Arc<super::#user::#user_ty>,
            #closed_field
        }
        impl #wrapper {
            fn __unibind_wrap(inner: super::#user::#user_ty) -> Self {
                Self {
                    inner: ::std::sync::Arc::new(inner),
                    #closed_init
                }
            }
        }
        #[::pyo3::pymethods]
        impl #wrapper {
            #constructor
            #(#methods)*
            #resource_surface
        }
        #leak_warning
    })
}

/// Constructors are sync and never `blocking` (lowering enforces both), so
/// only the throws and buffer aspects apply.
fn render_constructor(
    ctor: &ir::Function,
    object: &ir::Object,
    ctx: &Ctx<'_>,
) -> Result<TokenStream, RenderError> {
    let user = ctx.user;
    let object_ident = Ident::new(&object.name, Span::call_site());
    let ctor_name = Ident::new(&ctor.name, Span::call_site());
    let docs = doc_attrs(&ctor.docs);
    let args = sig::lower_args(ctor, ctx)?;
    let entries = &args.signature;
    let params = &args.params;
    let prologue = &args.prologue;
    let forwarded = &args.forwarded;
    let call = quote!(super::#user::#object_ident::#ctor_name(#(#forwarded),*));
    let body_and_ret = if ctor.throws.is_some() {
        sig::BodyAndRet {
            ret: quote!(::pyo3::PyResult<Self>),
            body: quote!(#call.map(Self::__unibind_wrap).map_err(::pyo3::PyErr::from)),
        }
    } else if args.fallible {
        // The buffer contiguity check can fail even though the user
        // constructor cannot.
        sig::BodyAndRet {
            ret: quote!(::pyo3::PyResult<Self>),
            body: quote!(::pyo3::PyResult::Ok(Self::__unibind_wrap(#call))),
        }
    } else {
        sig::BodyAndRet {
            ret: quote!(Self),
            body: quote!(Self::__unibind_wrap(#call)),
        }
    };
    let sig::BodyAndRet { ret, body } = body_and_ret;
    Ok(quote! {
        #docs
        #[new]
        #[pyo3(signature = (#(#entries),*))]
        fn __unibind_new(#(#params),*) -> #ret {
            #prologue
            #body
        }
    })
}
