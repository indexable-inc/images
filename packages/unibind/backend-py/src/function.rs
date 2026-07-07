//! Render wrappers around the user's callables: `#[pyfunction]`s for free
//! functions and `#[pymethods]` items for object methods. Sync, blocking
//! (GIL-released), and async bodies all route through the same argument
//! and return machinery in [`crate::sig`].

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::ctx::Ctx;
use crate::sig::{self, BodyAndRet};
use crate::RenderError;

/// Who the callable belongs to; the call target and receiver differ.
enum Target<'a> {
    /// A free `pub fn`: `super::<user>::<name>(...)`.
    Free,
    /// An object method: `self.inner.<name>(...)` sync, or the cloned
    /// `inner` Arc inside async futures.
    Method {
        /// The owning object's name, which scopes per-export stream
        /// classes.
        object: &'a str,
    },
}

impl Target<'_> {
    const fn owner(&self) -> Option<&str> {
        match self {
            Self::Free => None,
            Self::Method { object } => Some(object),
        }
    }

    fn sync_call(&self, name: &Ident, forwarded: &[TokenStream], user: &Ident) -> TokenStream {
        match self {
            Self::Free => quote!(super::#user::#name(#(#forwarded),*)),
            Self::Method { .. } => quote!(self.inner.#name(#(#forwarded),*)),
        }
    }

    /// Inside an async future the receiver is the cloned `inner` Arc: the
    /// future must be `'static`, so it cannot borrow `&self`.
    fn async_call(&self, name: &Ident, forwarded: &[TokenStream], user: &Ident) -> TokenStream {
        match self {
            Self::Free => quote!(super::#user::#name(#(#forwarded),*)),
            Self::Method { .. } => quote!(inner.#name(#(#forwarded),*)),
        }
    }
}

pub fn render_fn(function: &ir::Function, ctx: &Ctx<'_>) -> Result<TokenStream, RenderError> {
    render_callable(function, ctx, &Target::Free)
}

pub fn render_method(
    function: &ir::Function,
    ctx: &Ctx<'_>,
    object: &str,
) -> Result<TokenStream, RenderError> {
    render_callable(function, ctx, &Target::Method { object })
}

fn render_callable(
    function: &ir::Function,
    ctx: &Ctx<'_>,
    target: &Target<'_>,
) -> Result<TokenStream, RenderError> {
    let name = Ident::new(&function.name, Span::call_site());
    let rename = function.names.py.as_ref().map(|py_name| {
        quote! { #[pyo3(name = #py_name)] }
    });
    let docs = doc_attrs(&function.docs);
    let args = sig::lower_args(function, ctx)?;
    let ret = sig::ret_spec(function, target.owner(), ctx);
    let pyfunction = matches!(target, Target::Free).then(|| quote!(#[::pyo3::pyfunction]));
    let entries = &args.signature;
    let item = match function.asyncness {
        ir::Asyncness::Async => async_item(function, ctx, target, &name, &args, &ret),
        ir::Asyncness::Sync => sync_item(function, ctx, target, &name, &args, &ret),
    };
    Ok(quote! {
        #docs
        #pyfunction
        #rename
        #[pyo3(signature = (#(#entries),*))]
        #item
    })
}

fn sync_item(
    function: &ir::Function,
    ctx: &Ctx<'_>,
    target: &Target<'_>,
    name: &Ident,
    args: &sig::Args,
    ret: &sig::RetSpec,
) -> TokenStream {
    let call = target.sync_call(name, &args.forwarded, ctx.user);
    // `detach` releases the GIL around the user call; the prologue built
    // any buffer slices already and `&[u8]` is Send, so they cross into
    // the closure.
    let raw = if function.blocking {
        quote!(py.detach(move || #call))
    } else {
        call
    };
    let BodyAndRet { ret: ret_ty, body } =
        sig::finish_sync(&raw, function.throws.is_some(), args.fallible, ret);
    let prologue = &args.prologue;
    let params = &args.params;
    let mut lead = Vec::new();
    if matches!(target, Target::Method { .. }) {
        lead.push(quote!(&self));
    }
    if function.blocking {
        lead.push(quote!(py: ::pyo3::Python<'_>));
    }
    quote! {
        fn #name(#(#lead,)* #(#params),*) -> #ret_ty {
            #prologue
            #body
        }
    }
}

fn async_item(
    function: &ir::Function,
    ctx: &Ctx<'_>,
    target: &Target<'_>,
    name: &Ident,
    args: &sig::Args,
    ret: &sig::RetSpec,
) -> TokenStream {
    let call = target.async_call(name, &args.forwarded, ctx.user);
    let future_body = if function.throws.is_some() {
        ret.wrap.as_ref().map_or_else(
            || quote!(#call.await.map_err(::pyo3::PyErr::from)),
            |class| quote!(#call.await.map(#class::__unibind_wrap).map_err(::pyo3::PyErr::from)),
        )
    } else {
        let wrapped = sig::wrap_value(quote!(#call.await), ret.wrap.as_ref());
        quote!(::pyo3::PyResult::Ok(#wrapped))
    };
    let receiver = matches!(target, Target::Method { .. }).then(|| quote!(&self,));
    let clone_inner = matches!(target, Target::Method { .. }).then(|| {
        quote!(let inner = ::std::sync::Arc::clone(&self.inner);)
    });
    let params = &args.params;
    quote! {
        fn #name<'py>(
            #receiver
            py: ::pyo3::Python<'py>,
            #(#params),*
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            #clone_inner
            ::unibind_runtime::py::future_into_py(py, async move { #future_body })
        }
    }
}

pub fn doc_attrs(lines: &[String]) -> TokenStream {
    quote! { #(#[doc = #lines])* }
}
