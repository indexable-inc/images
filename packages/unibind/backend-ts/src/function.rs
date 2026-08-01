//! Render `#[napi]` wrappers around the user's functions and methods.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

use crate::ty::{self, Level, TyCtx};
use crate::{convert, defaults};

/// Where a callable sits. The receiver it declares and the scope its
/// stream class is named in both follow from it.
#[derive(Clone, Copy)]
pub enum Callee<'a> {
    /// A free `pub fn` in the exported module.
    Free,
    /// A method on `object`, whose name scopes the per-export stream class.
    Method { object: &'a str },
}

impl<'a> Callee<'a> {
    const fn owner(self) -> Option<&'a str> {
        match self {
            Self::Free => None,
            Self::Method { object } => Some(object),
        }
    }

    fn receiver(self) -> TokenStream {
        match self {
            Self::Free => TokenStream::new(),
            Self::Method { .. } => quote!(&self,),
        }
    }
}

/// The pieces of one callable's wrapper signature and call, shared between
/// free functions, object methods, and constructors.
pub struct Wrapper {
    /// `name: Type` parameter list entries, defaults already `Option`-ized.
    pub params: Vec<TokenStream>,
    /// Statements rebinding the arguments whose boundary shape differs from
    /// the user's own type (see [`crate::convert`]), run before the call.
    /// A non-empty prologue can refuse a value, so it also decides whether
    /// the wrapper returns a `napi::Result` where the user's function does
    /// not: the JavaScript surface is the same either way, since napi
    /// throws on `Err` and hands back the value on `Ok`.
    pub prologue: Vec<TokenStream>,
    /// Call-site expressions, index-aligned with the user's parameters.
    pub exprs: Vec<TokenStream>,
}

/// How one argument reaches the user's call: the statement that adapts what
/// JavaScript sent (only where the two sides spell the value differently)
/// and the expression the call site passes.
pub struct Binding {
    pub prologue: Option<TokenStream>,
    pub expr: TokenStream,
}

pub fn wrapper_parts(function: &ir::Function, ctx: &TyCtx<'_>) -> Result<Wrapper, RenderError> {
    let mut params = Vec::new();
    let mut prologue = Vec::new();
    let mut exprs = Vec::new();
    for arg in &function.args {
        ty::check(
            &arg.ty,
            &format!("argument `{}` of `{}`", arg.name, function.name),
        )?;
        let ident = name_ident(&arg.name)?;
        let declared = ty::decl(&arg.ty, ctx, Level::Top)?;
        let binding = match &arg.default {
            // An `Option` argument is already optional from JavaScript;
            // its default (implicit or explicit) substitutes in place.
            Some(default) if !matches!(arg.ty, ir::Type::Option(_)) => {
                params.push(quote!(#ident: ::std::option::Option<#declared>));
                defaults::defaulted(arg, default, &ident, function, ctx)?
            }
            Some(default) => {
                params.push(quote!(#ident: #declared));
                defaults::optional_defaulted(arg, default, &ident, function, ctx)?
            }
            None => {
                params.push(quote!(#ident: #declared));
                bind_plain(arg, &ident, ctx)
            }
        };
        prologue.extend(binding.prologue);
        exprs.push(binding.expr);
    }
    Ok(Wrapper {
        params,
        prologue,
        exprs,
    })
}

/// An argument with no declared default. Every adapted type crosses by
/// value, so the rebinding is the whole adaptation and `ty::pass` (which
/// only reborrows) has nothing left to add.
fn bind_plain(arg: &ir::Arg, ident: &proc_macro2::Ident, ctx: &TyCtx<'_>) -> Binding {
    let Some(converted) = convert::inward(&arg.ty, ctx, &quote!(#ident)) else {
        return Binding {
            prologue: None,
            expr: ty::pass(&arg.ty, &quote!(#ident)),
        };
    };
    Binding {
        prologue: Some(quote!(let #ident = #converted?;)),
        expr: quote!(#ident),
    }
}

/// Render one exported free function, including stream returns (whose
/// handle class renders separately in [`crate::stream`]).
///
/// A `blocking` export renders as a plain sync wrapper: `blocking` frees
/// Python's GIL, and JavaScript has no equivalent to free -- a sync export
/// occupies the event loop either way.
pub fn render_fn(function: &ir::Function, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let name = name_ident(&function.name)?;
    let user = ctx.user;
    let wrapper = wrapper_parts(function, ctx)?;
    let call = {
        let exprs = &wrapper.exprs;
        quote!(#user::#name(#(#exprs),*))
    };
    render_callable(function, ctx, &wrapper, &call, Callee::Free)
}

/// Render the shared wrapper shape around `call`.
pub fn render_callable(
    function: &ir::Function,
    ctx: &TyCtx<'_>,
    wrapper: &Wrapper,
    call: &TokenStream,
    callee: Callee<'_>,
) -> Result<TokenStream, RenderError> {
    if let Some(ret) = &function.ret {
        ty::check(ret, &format!("the return type of `{}`", function.name))?;
    }
    let name = name_ident(&function.name)?;
    let napi_attr = napi_attr(function.names.ts.as_deref());
    let docs = doc_attrs(&function.docs);
    let params = &wrapper.params;
    // A stream return crosses as the generated per-export handle class;
    // everything else declares through the shared type mapping.
    let ok_decl = match &function.ret {
        None => quote!(()),
        Some(ir::Type::Stream(_)) => {
            let class = ty::stream_class_ident(callee.owner(), &function.name);
            quote!(#class)
        }
        Some(ret) => ty::decl(ret, ctx, Level::Top)?,
    };
    let adapt = |value: &TokenStream| match &function.ret {
        None => value.clone(),
        Some(ir::Type::Stream(_)) => {
            let class = ty::stream_class_ident(callee.owner(), &function.name);
            quote!(#class::__unibind_from(#value))
        }
        Some(ret) => ty::ret(ret, ctx, value),
    };

    let receiver = callee.receiver();
    let shape = CallShape {
        name: &name,
        params,
        prologue: &wrapper.prologue,
        receiver: &receiver,
        call,
        ok_decl: &ok_decl,
        throws: function.throws.is_some(),
        fallible: !wrapper.prologue.is_empty(),
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
    prologue: &'a [TokenStream],
    /// `&self,` on a method, empty for a free function.
    receiver: &'a TokenStream,
    call: &'a TokenStream,
    ok_decl: &'a TokenStream,
    throws: bool,
    fallible: bool,
}

fn sync_body(shape: &CallShape<'_>, value: &TokenStream) -> BodyAndRet {
    let CallShape {
        name,
        params,
        prologue,
        receiver,
        call,
        ok_decl,
        throws,
        fallible,
    } = shape;
    let header = quote!(pub fn #name(#receiver #(#params),*));
    if *throws {
        BodyAndRet {
            header,
            ret: quote!(::napi::Result<#ok_decl>),
            body: quote! {
                #(#prologue)*
                match #call {
                    ::std::result::Result::Ok(value) => ::std::result::Result::Ok(#value),
                    ::std::result::Result::Err(error) => {
                        ::std::result::Result::Err(::napi::Error::from(error))
                    }
                }
            },
        }
    } else if *fallible {
        BodyAndRet {
            header,
            ret: quote!(::napi::Result<#ok_decl>),
            body: quote! {
                #(#prologue)*
                let value = #call;
                ::std::result::Result::Ok(#value)
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
/// race the user future against the abort notification through the
/// module's shared `__unibind_with_abort` (see [`crate::module`]); only
/// the settle conversion stays per binding.
fn async_body(shape: &CallShape<'_>, value: &TokenStream) -> BodyAndRet {
    let CallShape {
        name,
        params,
        prologue,
        receiver,
        call,
        ok_decl,
        throws,
        fallible: _,
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
            #(#prologue)*
            let value = __unibind_with_abort(__unibind_signal, #call).await?;
            #settle
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
