//! Render `#[wasm_bindgen]` wrappers around the user's functions and methods.
//!
//! Nothing here renders an `async fn`. An async export becomes a *sync* fn
//! handing back a `js_sys::Promise` built by
//! `wasm_bindgen_futures::future_to_promise`: `wasm-bindgen`'s support for an
//! `async fn` with a `&self` receiver inside an exported impl is
//! version-dependent, and a `Promise` is what the JavaScript caller receives
//! either way. The consequence to keep in mind while reading the async arm is
//! that the future is `'static`: it cannot borrow `self`, so an object's
//! wrapper clones the `Arc` out first ([`Call::prelude`]), and the argument
//! conversions run *inside* the future, where a refusal is the rejection the
//! caller would have got from a throwing `async fn`.

use proc_macro2::{Ident, TokenStream};
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

use crate::ty::{self, Level, TyCtx};
use crate::{convert, defaults, error, names};

/// Where a callable sits. The receiver it declares, the attribute that marks
/// it, and the scope its stream class is named in all follow from it.
#[derive(Clone, Copy)]
pub enum Callee<'a> {
    /// A free `pub fn` in the exported module.
    Free,
    /// A method on `object`, whose name scopes the per-export stream class.
    Method { object: &'a str },
    /// A function on `object` rather than on an instance. No receiver, but it
    /// still scopes stream classes by the object, and `wasm-bindgen` makes a
    /// receiver-less function in an exported impl a static method with no
    /// attribute of its own.
    Associated { object: &'a str },
}

impl<'a> Callee<'a> {
    const fn owner(self) -> Option<&'a str> {
        match self {
            Self::Free => None,
            Self::Method { object } | Self::Associated { object } => Some(object),
        }
    }

    fn receiver(self) -> TokenStream {
        match self {
            // No receiver: one of these may be what runs before an instance
            // exists at all.
            Self::Free | Self::Associated { .. } => TokenStream::new(),
            Self::Method { .. } => quote!(&self,),
        }
    }

    /// The attribute naming one export on the JavaScript side.
    ///
    /// A member's option list belongs to the enclosing impl's own attribute,
    /// which parses and strips it; a fully qualified path there is not one of
    /// the options it recognizes, so only a free function spells the macro.
    fn attr(self, js_name: &str) -> TokenStream {
        match self {
            Self::Free => quote! {
                #[::wasm_bindgen::prelude::wasm_bindgen(js_name = #js_name)]
            },
            Self::Method { .. } | Self::Associated { .. } => quote! {
                #[wasm_bindgen(js_name = #js_name)]
            },
        }
    }
}

/// The pieces of one callable's wrapper signature and call, shared between free
/// functions, object methods, and constructors.
pub struct Wrapper {
    /// `name: Type` parameter list entries, defaults already `Option`-ized.
    pub params: Vec<TokenStream>,
    /// Statements rebinding the arguments whose boundary shape differs from the
    /// user's own type (see [`crate::convert`]). A non-empty prologue can
    /// refuse a value, so it also decides whether a sync wrapper returns a
    /// `Result` where the user's function does not: the JavaScript surface is
    /// the same either way, since `wasm-bindgen` throws on `Err`.
    pub prologue: Vec<TokenStream>,
    /// Call-site expressions, index-aligned with the user's parameters.
    pub exprs: Vec<TokenStream>,
}

/// How one argument reaches the user's call: the statement that adapts what
/// JavaScript sent (only where the two sides spell the value differently) and
/// the expression the call site passes.
pub struct Binding {
    pub prologue: Option<TokenStream>,
    pub expr: TokenStream,
}

/// The user call one wrapper wraps.
pub struct Call {
    /// Statements that must run before the future is spawned, because a
    /// `'static` future cannot borrow `&self`: cloning a method's `Arc` out of
    /// the handle. Empty for everything sync and for every free function.
    pub prelude: TokenStream,
    /// The call itself: the user's function for a sync export, a future for an
    /// async one.
    pub expr: TokenStream,
}

/// One adapted value with its refusal wired into the wrapper's own `Result`:
/// the conversion's reason string becomes the `JsValue` the boundary throws.
/// Written here rather than in each caller so the `?` and the wrap have one
/// spelling.
pub fn refused(converted: &TokenStream) -> TokenStream {
    quote!(#converted.map_err(__unibind_wasm_error)?)
}

/// # Errors
///
/// Fails for an argument type the wasm boundary cannot carry, a name that
/// cannot become an identifier, or a default the argument's type cannot take.
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
            // An `Option` argument is already optional from JavaScript; its
            // default (implicit or explicit) substitutes in place.
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
                bind_plain(arg, &ident, ctx)?
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

/// An argument with no declared default. Every adapted type crosses by value,
/// so the rebinding is the whole adaptation and [`ty::pass`] (which only
/// reborrows) has nothing left to add.
fn bind_plain(arg: &ir::Arg, ident: &Ident, ctx: &TyCtx<'_>) -> Result<Binding, RenderError> {
    let Some(converted) = convert::inward(&arg.ty, ctx, &quote!(#ident))? else {
        return Ok(Binding {
            prologue: None,
            expr: ty::pass(&arg.ty, &quote!(#ident)),
        });
    };
    let converted = refused(&converted);
    Ok(Binding {
        prologue: Some(quote!(let #ident = #converted;)),
        expr: quote!(#ident),
    })
}

/// Render one exported free function, including stream returns (whose handle
/// class renders separately in [`crate::stream`]).
///
/// # Errors
///
/// Fails for surface the wasm backend does not carry; see [`render_callable`].
pub fn render_fn(function: &ir::Function, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let name = name_ident(&function.name)?;
    let user = ctx.user;
    let wrapper = wrapper_parts(function, ctx)?;
    let exprs = &wrapper.exprs;
    let call = Call {
        prelude: TokenStream::new(),
        expr: quote!(#user::#name(#(#exprs),*)),
    };
    render_callable(function, ctx, &wrapper, &call, Callee::Free)
}

/// Render the shared wrapper shape around `call`.
///
/// # Errors
///
/// Fails for a return type the wasm boundary cannot carry (an integer-keyed
/// map) or a name that cannot become an identifier.
pub fn render_callable(
    function: &ir::Function,
    ctx: &TyCtx<'_>,
    wrapper: &Wrapper,
    call: &Call,
    callee: Callee<'_>,
) -> Result<TokenStream, RenderError> {
    if let Some(ret) = &function.ret {
        ty::check(ret, &format!("the return type of `{}`", function.name))?;
    }
    let name = name_ident(&function.name)?;
    let js_name = names::js_member(&function.names, &function.name);
    let settle = settle(function, ctx, callee)?;
    let shape = CallShape {
        name: &name,
        params: &wrapper.params,
        prologue: &wrapper.prologue,
        receiver: &callee.receiver(),
        call,
        ret: function.ret.as_ref(),
        settle: settle.as_ref(),
        throws: function.throws.as_deref().map(error::conversion_ident),
        ctx,
    };
    let BodyAndRet { header, ret, body } = match function.asyncness {
        ir::Asyncness::Sync => sync_body(&shape),
        ir::Asyncness::Async => async_body(&shape),
    };
    let docs = doc_attrs(&function.docs);
    let attr = callee.attr(&js_name);
    Ok(quote! {
        #docs
        #attr
        #header -> #ret {
            #body
        }
    })
}

/// How the callable's success value reaches JavaScript; `None` for a unit
/// return, which has no value to convert.
fn settle(
    function: &ir::Function,
    ctx: &TyCtx<'_>,
    callee: Callee<'_>,
) -> Result<Option<ty::Returned>, RenderError> {
    match &function.ret {
        None => Ok(None),
        // A stream crosses as the generated per-export handle class, which
        // `crate::stream` renders alongside rather than inside the object.
        Some(ir::Type::Stream(_)) => {
            let class = ty::stream_class_ident(callee.owner(), &function.name);
            Ok(Some(ty::Returned {
                decl: quote!(#class),
                value: quote!(#class::__unibind_from(value)),
                fallible: false,
            }))
        }
        Some(ret) => ty::returned(ret, ctx).map(Some),
    }
}

/// A wrapper's signature header, return type, and body, which vary together on
/// asyncness, `throws`, and whether anything can refuse.
struct BodyAndRet {
    header: TokenStream,
    ret: TokenStream,
    body: TokenStream,
}

/// Everything the body builders need to spell one wrapper.
struct CallShape<'a> {
    name: &'a Ident,
    params: &'a [TokenStream],
    prologue: &'a [TokenStream],
    /// `&self,` on a method, empty otherwise.
    receiver: &'a TokenStream,
    call: &'a Call,
    ret: Option<&'a ir::Type>,
    settle: Option<&'a ty::Returned>,
    /// The generated conversion for the error enum the user's `Result` names.
    throws: Option<Ident>,
    ctx: &'a TyCtx<'a>,
}

impl CallShape<'_> {
    /// The declared success type.
    fn ok_decl(&self) -> TokenStream {
        self.settle
            .map_or_else(|| quote!(()), |settle| settle.decl.clone())
    }

    /// The success value as the declaration spells it, from `value`.
    ///
    /// Yields a `Result<_, JsValue>` whenever the conversion can refuse, so
    /// both arms below read the same at the call site.
    fn ok_value(&self) -> TokenStream {
        self.settle.map_or_else(
            || quote!(::std::result::Result::Ok(())),
            |settle| {
                let value = &settle.value;
                if settle.fallible {
                    quote!(#value.map_err(__unibind_wasm_error))
                } else {
                    quote!(::std::result::Result::Ok(#value))
                }
            },
        )
    }

    /// The `Result<JsValue, JsValue>` a `Promise` settles with, from `value`.
    fn resolved(&self) -> TokenStream {
        ty::resolved(self.ret, self.ctx, self.settle)
    }

    /// Whether a sync wrapper hands back a `Result` where the user's function
    /// does not: an argument conversion or the return conversion can refuse.
    fn fallible(&self) -> bool {
        !self.prologue.is_empty() || self.settle.is_some_and(|settle| settle.fallible)
    }
}

fn sync_body(shape: &CallShape<'_>) -> BodyAndRet {
    let CallShape {
        name,
        params,
        prologue,
        receiver,
        call,
        ..
    } = shape;
    let Call { prelude, expr } = call;
    let header = quote!(pub fn #name(#receiver #(#params),*));
    let ok_decl = shape.ok_decl();
    let ok_value = shape.ok_value();
    if let Some(throws) = &shape.throws {
        let ok_arm = shape.settle.map_or_else(
            || quote!(::std::result::Result::Ok(()) => #ok_value,),
            |_| quote!(::std::result::Result::Ok(value) => #ok_value,),
        );
        return BodyAndRet {
            header,
            ret: quote!(::std::result::Result<#ok_decl, ::wasm_bindgen::JsValue>),
            body: quote! {
                #prelude
                #(#prologue)*
                match #expr {
                    #ok_arm
                    ::std::result::Result::Err(error) => {
                        ::std::result::Result::Err(#throws(error))
                    }
                }
            },
        };
    }
    if shape.fallible() {
        let bind = shape.settle.map_or_else(
            || quote!(#expr;),
            |_| quote!(let value = #expr;),
        );
        return BodyAndRet {
            header,
            ret: quote!(::std::result::Result<#ok_decl, ::wasm_bindgen::JsValue>),
            body: quote! {
                #prelude
                #(#prologue)*
                #bind
                #ok_value
            },
        };
    }
    let body = shape.settle.map_or_else(
        || quote!(#expr;),
        |settle| {
            let value = &settle.value;
            quote! {
                let value = #expr;
                #value
            }
        },
    );
    BodyAndRet {
        header,
        ret: ok_decl,
        body: quote! {
            #prelude
            #body
        },
    }
}

/// The async wrapper: clone whatever the future must own out of `&self`, then
/// spawn one that converts the arguments, races the user future against the
/// abort notification through the module's shared `__unibind_wasm_with_abort`
/// (see [`crate::module`]), and settles into a `JsValue` either way.
fn async_body(shape: &CallShape<'_>) -> BodyAndRet {
    let CallShape {
        name,
        params,
        prologue,
        receiver,
        call,
        ..
    } = shape;
    let Call { prelude, expr } = call;
    let awaited = quote!(__unibind_wasm_with_abort(__unibind_signal, #expr).await?);
    let resolved = shape.resolved();
    let settled = match (&shape.throws, shape.settle.is_some()) {
        (Some(throws), true) => quote! {
            match #awaited {
                ::std::result::Result::Ok(value) => #resolved,
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(#throws(error))
                }
            }
        },
        (Some(throws), false) => quote! {
            match #awaited {
                ::std::result::Result::Ok(()) => #resolved,
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(#throws(error))
                }
            }
        },
        (None, true) => quote! {
            let value = #awaited;
            #resolved
        },
        (None, false) => quote! {
            #awaited;
            #resolved
        },
    };
    BodyAndRet {
        header: quote! {
            pub fn #name(
                #receiver
                #(#params,)*
                __unibind_signal: ::std::option::Option<::js_sys::Object>,
            )
        },
        ret: quote!(::js_sys::Promise),
        body: quote! {
            #prelude
            ::wasm_bindgen_futures::future_to_promise(async move {
                #(#prologue)*
                #settled
            })
        },
    }
}

pub fn doc_attrs(lines: &[String]) -> TokenStream {
    quote! { #(#[doc = #lines])* }
}
