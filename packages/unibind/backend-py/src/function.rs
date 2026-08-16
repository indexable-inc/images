//! Render wrappers around the user's callables: `#[pyfunction]`s for free
//! functions and `#[pymethods]` items for object methods. Sync, blocking
//! (GIL-released), and async bodies all route through the same argument
//! and return machinery in [`crate::sig`].

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;
use unibind_core::render::RenderError;

use crate::ctx::Ctx;
use crate::sig::{self, BodyAndRet};

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
    /// A `#[unibind(associated)]` function:
    /// `super::<user>::<Object>::<name>(...)`, rendered as a
    /// `@staticmethod`. No receiver, and it may be async, which is the
    /// shape `__new__` cannot take.
    Associated {
        /// The owning object's Rust name, which both spells the call and
        /// scopes per-export stream classes.
        object: &'a str,
    },
}

impl Target<'_> {
    const fn owner(&self) -> Option<&str> {
        match self {
            Self::Free => None,
            Self::Method { object } | Self::Associated { object } => Some(object),
        }
    }

    fn sync_call(&self, call: CallParts<'_>) -> TokenStream {
        let CallParts {
            name,
            forwarded,
            user,
        } = call;
        match self {
            Self::Free => quote!(super::#user::#name(#(#forwarded),*)),
            Self::Method { .. } => quote!(self.inner.#name(#(#forwarded),*)),
            Self::Associated { object } => {
                let object = Ident::new(object, Span::call_site());
                quote!(super::#user::#object::#name(#(#forwarded),*))
            }
        }
    }

    /// Inside an async future the receiver is the cloned `inner` Arc: the
    /// future must be `'static`, so it cannot borrow `&self`.
    fn async_call(&self, call: CallParts<'_>) -> TokenStream {
        let CallParts {
            name,
            forwarded,
            user,
        } = call;
        match self {
            Self::Free => quote!(super::#user::#name(#(#forwarded),*)),
            Self::Method { .. } => quote!(inner.#name(#(#forwarded),*)),
            Self::Associated { object } => {
                let object = Ident::new(object, Span::call_site());
                quote!(super::#user::#object::#name(#(#forwarded),*))
            }
        }
    }
}

/// The user-side call a rendered wrapper forwards to: the callee's Rust ident,
/// the already-lowered argument expressions, and the user module's ident.
///
/// `name` and `user` are both `&Ident`; naming them keeps a transposed pair
/// from compiling into `super::close::my_module(..)`.
/// `Copy` because both renderers only read it; by value without `Copy` reads
/// to `clippy::needless_pass_by_value` as a move that never happens.
#[derive(Clone, Copy)]
struct CallParts<'a> {
    name: &'a Ident,
    forwarded: &'a [TokenStream],
    user: &'a Ident,
}

/// Everything one rendered pyo3 item needs, gathered once by
/// [`render_callable`] and consumed by whichever of the sync/async renderers
/// the callable's asyncness selects.
///
/// `Copy` for the same reason as [`CallParts`].
#[derive(Clone, Copy)]
struct ItemParts<'a, 'ctx> {
    function: &'a ir::Function,
    ctx: &'a Ctx<'ctx>,
    target: &'a Target<'ctx>,
    /// The wrapper's Rust ident, shared by the item signature and the call.
    name: &'a Ident,
    args: &'a sig::Args,
    ret: &'a sig::RetSpec,
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

/// A `#[unibind(associated)]` function, as a `@staticmethod`.
///
/// # Errors
///
/// Fails for the same type surface any callable refuses.
pub fn render_associated(
    function: &ir::Function,
    ctx: &Ctx<'_>,
    object: &str,
) -> Result<TokenStream, RenderError> {
    render_callable(function, ctx, &Target::Associated { object })
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
    // pyo3 composes `staticmethod` with an async fn, which is what makes
    // these expressible where `#[new]` is not.
    let staticmethod = matches!(target, Target::Associated { .. }).then(|| quote!(#[staticmethod]));
    let entries = &args.signature;
    let parts = ItemParts {
        function,
        ctx,
        target,
        name: &name,
        args: &args,
        ret: &ret,
    };
    let item = match function.asyncness {
        ir::Asyncness::Async => async_item(parts),
        ir::Asyncness::Sync => sync_item(parts),
    };
    Ok(quote! {
        #docs
        #pyfunction
        #staticmethod
        #rename
        #[pyo3(signature = (#(#entries),*))]
        #item
    })
}

fn sync_item(parts: ItemParts<'_, '_>) -> TokenStream {
    let ItemParts {
        function,
        ctx,
        target,
        name,
        args,
        ret,
    } = parts;
    let call = target.sync_call(CallParts {
        name,
        forwarded: &args.forwarded,
        user: ctx.user,
    });
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

fn async_item(parts: ItemParts<'_, '_>) -> TokenStream {
    let ItemParts {
        function,
        ctx,
        target,
        name,
        args,
        ret,
    } = parts;
    let call = target.async_call(CallParts {
        name,
        forwarded: &args.forwarded,
        user: ctx.user,
    });
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
    let clone_inner = matches!(target, Target::Method { .. })
        .then(|| quote!(let inner = ::std::sync::Arc::clone(&self.inner);));
    let params = &args.params;
    quote! {
        fn #name<'py>(
            #receiver
            py: ::pyo3::Python<'py>,
            #(#params),*
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            #clone_inner
            ::unibind_py_runtime::future_into_py(py, async move { #future_body })
        }
    }
}

pub fn doc_attrs(lines: &[String]) -> TokenStream {
    quote! { #(#[doc = #lines])* }
}
