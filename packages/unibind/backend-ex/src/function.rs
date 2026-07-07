//! Render `#[rustler::nif]` wrappers around the user's functions.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::{error, names, ty, RenderError};

pub fn render_fn(function: &ir::Function, user: &Ident) -> Result<TokenStream, RenderError> {
    if matches!(function.asyncness, ir::Asyncness::Async) {
        if matches!(function.ret, Some(ir::Type::Stream(_))) {
            return Err(RenderError::new(format!(
                "`{}` is an async fn returning a stream; the elixir backend \
                 drives streams from a plain fn, so return `UniStream<T>` \
                 without `async` (the producer already runs on the shared \
                 runtime)",
                function.name
            )));
        }
        return render_async_fn(function, user);
    }
    if let Some(ir::Type::Stream(item)) = &function.ret {
        return render_stream_fn(function, item, user);
    }
    render_sync_fn(function, user)
}

/// The `#[rustler::nif(...)]` attribute: `DirtyIo` scheduling for blocking
/// calls, and the registered name when it differs from the wrapper's.
pub fn nif_attr(function: &ir::Function, nif_name: &str, wrapper: &Ident) -> TokenStream {
    let mut options = Vec::new();
    if function.blocking {
        options.push(quote!(schedule = "DirtyIo"));
    }
    if wrapper != nif_name {
        options.push(quote!(name = #nif_name));
    }
    if options.is_empty() {
        quote!(#[::rustler::nif])
    } else {
        quote!(#[::rustler::nif(#(#options),*)])
    }
}

/// Wrapper parameters and the matching call-site forwards.
pub struct Params {
    pub params: Vec<TokenStream>,
    pub forwarded: Vec<TokenStream>,
}

/// Parameters spelled as the IR declares them; borrows decode zero-copy
/// from the calling term.
pub fn borrowed_params(function: &ir::Function, user: &Ident) -> Result<Params, RenderError> {
    let mut params = Vec::new();
    let mut forwarded = Vec::new();
    for arg in &function.args {
        let ident = names::name_ident(&names::ex_arg_name(arg))?;
        let spelled = ty::rust_type(&arg.ty, user).map_err(|error| at_arg(arg, &error))?;
        params.push(quote!(#ident: #spelled));
        forwarded.push(quote!(#ident));
    }
    Ok(Params { params, forwarded })
}

/// Parameters owned outright, re-borrowed at the call site: async wrappers
/// move their arguments into a `'static` future.
fn owned_params(function: &ir::Function, user: &Ident) -> Result<Params, RenderError> {
    let mut params = Vec::new();
    let mut forwarded = Vec::new();
    for arg in &function.args {
        let ident = names::name_ident(&names::ex_arg_name(arg))?;
        let spelled = ty::owned_type(&arg.ty, user).map_err(|error| at_arg(arg, &error))?;
        params.push(quote!(#ident: #spelled));
        forwarded.push(ty::reborrow(&ident, &arg.ty));
    }
    Ok(Params { params, forwarded })
}

fn at_arg(arg: &ir::Arg, error: &RenderError) -> RenderError {
    RenderError::new(format!("argument `{}`: {}", arg.name, error.message))
}

fn render_sync_fn(function: &ir::Function, user: &Ident) -> Result<TokenStream, RenderError> {
    let rust_name = Ident::new(&function.name, Span::call_site());
    let attr = nif_attr(function, &names::ex_fn_name(function), &rust_name);
    let Params { params, forwarded } = borrowed_params(function, user)?;
    let ok_ty = function.ret.as_ref().map_or_else(
        || Ok(quote!(())),
        |ret| {
            ty::rust_type(ret, user).map_err(|error| {
                RenderError::new(format!("return of `{}`: {}", function.name, error.message))
            })
        },
    )?;
    let call = quote!(super::#user::#rust_name(#(#forwarded),*));
    let signature_and_body = if let Some(throws) = &function.throws {
        let term = error::term_ident(throws);
        SignatureAndBody {
            ret: quote!(::std::result::Result<#ok_ty, #term>),
            body: quote!(#call.map_err(#term::from)),
        }
    } else {
        SignatureAndBody {
            ret: ok_ty,
            body: call,
        }
    };
    let SignatureAndBody { ret, body } = signature_and_body;
    Ok(quote! {
        #attr
        fn #rust_name(#(#params),*) -> #ret {
            #body
        }
    })
}

/// A wrapper's return type and body, which vary together on `throws`.
struct SignatureAndBody {
    ret: TokenStream,
    body: TokenStream,
}

/// An async wrapper takes the caller's reply reference, spawns the future
/// on the shared runtime, and returns the in-flight handle. The reply
/// arrives as `{:unibind, ref, {:ok, _} | {:error, _}}`.
fn render_async_fn(function: &ir::Function, user: &Ident) -> Result<TokenStream, RenderError> {
    let rust_name = Ident::new(&function.name, Span::call_site());
    let attr = nif_attr(function, &names::ex_fn_name(function), &rust_name);
    let Params { params, forwarded } = owned_params(function, user)?;
    if let Some(ret) = &function.ret {
        ty::check_boundary(ret).map_err(|error| {
            RenderError::new(format!("return of `{}`: {}", function.name, error.message))
        })?;
    }
    let call = quote!(super::#user::#rust_name(#(#forwarded),*).await);
    let fut = function.throws.as_ref().map_or_else(
        || {
            quote! {
                async move {
                    ::std::result::Result::<_, ::unibind_ex_runtime::Never>::Ok(#call)
                }
            }
        },
        |throws| {
            let term = error::term_ident(throws);
            quote!(async move { #call.map_err(#term::from) })
        },
    );
    Ok(quote! {
        #attr
        fn #rust_name(
            env: ::rustler::Env,
            reference: ::rustler::Term,
            #(#params),*
        ) -> ::rustler::NifResult<::rustler::ResourceArc<::unibind_ex_runtime::InFlight>> {
            let fut = #fut;
            ::unibind_ex_runtime::spawn_reply(env, reference, fut)
        }
    })
}

/// A stream wrapper calls the user function synchronously and hands the
/// stream to the runtime, which sends `{:unibind_stream, ref, {:item, _}}`
/// per granted credit and `{:unibind_stream, ref, :done}` at the end. The
/// returned handle takes demand through the `unibind_demand` NIF.
fn render_stream_fn(
    function: &ir::Function,
    item: &ir::Type,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    let rust_name = Ident::new(&function.name, Span::call_site());
    let attr = nif_attr(function, &names::ex_fn_name(function), &rust_name);
    let Params { params, forwarded } = borrowed_params(function, user)?;
    ty::check_boundary(item).map_err(|error| {
        RenderError::new(format!(
            "stream item of `{}`: {}",
            function.name, error.message
        ))
    })?;
    let call = quote!(super::#user::#rust_name(#(#forwarded),*));
    if let Some(throws) = &function.throws {
        let term = error::term_ident(throws);
        return Ok(quote! {
            #attr
            fn #rust_name(
                env: ::rustler::Env,
                reference: ::rustler::Term,
                #(#params),*
            ) -> ::rustler::NifResult<
                ::std::result::Result<
                    ::rustler::ResourceArc<::unibind_ex_runtime::StreamHandle>,
                    #term,
                >,
            > {
                match #call {
                    ::std::result::Result::Ok(stream) => {
                        ::unibind_ex_runtime::spawn_stream(env, reference, stream).map(Ok)
                    }
                    ::std::result::Result::Err(error) => Ok(Err(#term::from(error))),
                }
            }
        });
    }
    Ok(quote! {
        #attr
        fn #rust_name(
            env: ::rustler::Env,
            reference: ::rustler::Term,
            #(#params),*
        ) -> ::rustler::NifResult<::rustler::ResourceArc<::unibind_ex_runtime::StreamHandle>> {
            ::unibind_ex_runtime::spawn_stream(env, reference, #call)
        }
    })
}

/// The demand NIF, emitted once when any stream function exists.
pub fn demand_nif() -> TokenStream {
    quote! {
        #[::rustler::nif]
        fn unibind_demand(
            handle: ::rustler::ResourceArc<::unibind_ex_runtime::StreamHandle>,
            n: u64,
        ) {
            ::unibind_ex_runtime::grant(&handle, n);
        }
    }
}
