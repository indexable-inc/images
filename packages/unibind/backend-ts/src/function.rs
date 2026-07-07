//! Render `#[napi]` wrappers around the user's functions and methods.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;

use crate::ty::{self, Level, TyCtx};
use crate::{defaults, RenderError};

/// The pieces of one callable's wrapper signature and call, shared between
/// free functions, object methods, and constructors.
pub struct Wrapper {
    /// `name: Type` parameter list entries, defaults already `Option`-ized.
    pub params: Vec<TokenStream>,
    /// Call-site expressions, index-aligned with the user's parameters.
    pub exprs: Vec<TokenStream>,
}

pub fn wrapper_parts(function: &ir::Function, ctx: &TyCtx<'_>) -> Result<Wrapper, RenderError> {
    let mut params = Vec::new();
    let mut exprs = Vec::new();
    for arg in &function.args {
        ty::check(&arg.ty, &format!("argument `{}` of `{}`", arg.name, function.name))?;
        let ident = ty::name_ident(&arg.name)?;
        let declared = ty::decl(&arg.ty, ctx, Level::Top)?;
        match &arg.default {
            // An `Option` argument is already optional from JavaScript;
            // its default (implicit or explicit) substitutes in place.
            Some(default) if !matches!(arg.ty, ir::Type::Option(_)) => {
                params.push(quote!(#ident: ::std::option::Option<#declared>));
                exprs.push(defaults::substituted(arg, default, &ident, function)?);
            }
            Some(default) => {
                params.push(quote!(#ident: #declared));
                exprs.push(defaults::option_substituted(arg, default, &ident, function)?);
            }
            None => {
                params.push(quote!(#ident: #declared));
                exprs.push(ty::pass(&arg.ty, &quote!(#ident)));
            }
        }
    }
    Ok(Wrapper { params, exprs })
}

/// Render one exported free function, including stream returns (whose
/// handle class renders separately in [`crate::stream`]).
///
/// A `blocking` export renders as a plain sync wrapper: `blocking` frees
/// Python's GIL, and JavaScript has no equivalent to free -- a sync export
/// occupies the event loop either way.
pub fn render_fn(function: &ir::Function, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let name = ty::name_ident(&function.name)?;
    let user = ctx.user;
    let wrapper = wrapper_parts(function, ctx)?;
    let call = {
        let exprs = &wrapper.exprs;
        quote!(super::#user::#name(#(#exprs),*))
    };
    render_callable(function, ctx, &wrapper, &call, None)
}

/// Render the shared wrapper shape around `call`. `receiver` carries the
/// extra tokens object methods prepend to the parameter list.
pub fn render_callable(
    function: &ir::Function,
    ctx: &TyCtx<'_>,
    wrapper: &Wrapper,
    call: &TokenStream,
    receiver: Option<&TokenStream>,
) -> Result<TokenStream, RenderError> {
    if let Some(ret) = &function.ret {
        ty::check(ret, &format!("the return type of `{}`", function.name))?;
    }
    let name = ty::name_ident(&function.name)?;
    let napi_attr = napi_attr(function.names.ts.as_deref());
    let docs = doc_attrs(&function.docs);
    let params = &wrapper.params;
    // A stream return crosses as the generated per-function handle class;
    // everything else declares through the shared type mapping.
    let ok_decl = match &function.ret {
        None => quote!(()),
        Some(ir::Type::Stream(_)) => {
            let class = ty::stream_class_ident(&function.name);
            quote!(#class)
        }
        Some(ret) => ty::decl(ret, ctx, Level::Top)?,
    };
    let adapt = |value: &TokenStream| match &function.ret {
        None => value.clone(),
        Some(ir::Type::Stream(_)) => {
            let class = ty::stream_class_ident(&function.name);
            quote!(#class::__unibind_from(#value))
        }
        Some(ret) => ty::ret(ret, ctx, value),
    };

    let shape = CallShape {
        name: &name,
        params,
        receiver,
        call,
        ok_decl: &ok_decl,
        throws: function.throws.is_some(),
    };
    let body_and_ret = match function.asyncness {
        ir::Asyncness::Sync => sync_body(&shape, &adapt(&quote!(value))),
        ir::Asyncness::Async => async_body(&shape, &adapt(&quote!(value))),
    };
    let BodyAndRet { header, ret, body } = body_and_ret;
    Ok(quote! {
        #docs
        #napi_attr
        #header -> #ret {
            #body
        }
    })
}

/// A wrapper's signature header, return type, and body, which vary
/// together on asyncness and `throws`.
struct BodyAndRet {
    header: TokenStream,
    ret: TokenStream,
    body: TokenStream,
}

/// Everything the body builders need to spell one wrapper.
struct CallShape<'a> {
    name: &'a proc_macro2::Ident,
    params: &'a [TokenStream],
    receiver: Option<&'a TokenStream>,
    call: &'a TokenStream,
    ok_decl: &'a TokenStream,
    throws: bool,
}

fn sync_body(shape: &CallShape<'_>, value: &TokenStream) -> BodyAndRet {
    let CallShape {
        name,
        params,
        receiver,
        call,
        ok_decl,
        throws,
    } = shape;
    let header = quote!(pub fn #name(#receiver #(#params),*));
    if *throws {
        BodyAndRet {
            header,
            ret: quote!(::napi::Result<#ok_decl>),
            body: quote! {
                match #call {
                    ::std::result::Result::Ok(value) => ::std::result::Result::Ok(#value),
                    ::std::result::Result::Err(error) => {
                        ::std::result::Result::Err(::napi::Error::from(error))
                    }
                }
            },
        }
    } else {
        BodyAndRet {
            header,
            ret: (*ok_decl).clone(),
            body: quote! {
                let value = #call;
                #value
            },
        }
    }
}

/// The async wrapper: convert arguments on the JavaScript thread, then
/// `select!` the user future against the abort notification so an abort
/// drops the future.
fn async_body(shape: &CallShape<'_>, value: &TokenStream) -> BodyAndRet {
    let CallShape {
        name,
        params,
        receiver,
        call,
        ok_decl,
        throws,
    } = shape;
    let settle = if *throws {
        quote! {
            match value {
                ::std::result::Result::Ok(value) => ::std::result::Result::Ok(#value),
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(::napi::Error::from(error))
                }
            }
        }
    } else {
        quote!(::std::result::Result::Ok(#value))
    };
    BodyAndRet {
        header: quote! {
            pub async fn #name(
                #receiver
                #(#params,)*
                __unibind_signal: ::std::option::Option<__UnibindAbortSignal>,
            )
        },
        ret: quote!(::napi::Result<#ok_decl>),
        body: quote! {
            let __unibind_future = #call;
            match __unibind_signal {
                ::std::option::Option::Some(__unibind_signal) => {
                    if __unibind_signal.already_aborted {
                        return ::std::result::Result::Err(__unibind_aborted());
                    }
                    ::tokio::select! {
                        biased;
                        () = __unibind_signal.notify.notified() => {
                            ::std::result::Result::Err(__unibind_aborted())
                        }
                        value = __unibind_future => #settle,
                    }
                }
                ::std::option::Option::None => {
                    let value = __unibind_future.await;
                    #settle
                }
            }
        },
    }
}

pub fn doc_attrs(lines: &[String]) -> TokenStream {
    quote! { #(#[doc = #lines])* }
}

/// The `#[napi]` marker, with `js_name` folded in on rename. One combined
/// attribute: napi reads exactly one option list per item. The marker is
/// load-bearing on impl methods: napi's impl expansion skips methods
/// without their own `#[napi]` attribute.
pub fn napi_attr(ts_name: Option<&str>) -> TokenStream {
    ts_name.map_or_else(
        || quote!(#[::napi_derive::napi]),
        |js_name| quote!(#[::napi_derive::napi(js_name = #js_name)]),
    )
}
