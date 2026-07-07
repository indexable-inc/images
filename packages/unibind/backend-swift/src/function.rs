//! Render the wrapper function the bridge dispatches to for each exported
//! function: bridge reprs in, user types at the call, bridge reprs out.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::error;
use crate::names;
use crate::repr;
use crate::RenderError;

/// One rendered function: its declaration inside the bridge's
/// `extern "Rust"` block and its wrapper in the glue module.
pub struct RenderedFunction {
    pub bridge_decl: TokenStream,
    pub item: TokenStream,
}

/// The wrapper's name (`__unibind_fn_rows`); the ergonomic Swift name is the
/// overlay's business.
pub fn wrapper_ident(function: &ir::Function) -> Ident {
    Ident::new(&format!("__unibind_fn_{}", function.name), Span::call_site())
}

pub fn render_fn(
    function: &ir::Function,
    interface: &ir::Interface,
    ffi_mod: &Ident,
    user: &Ident,
) -> Result<RenderedFunction, RenderError> {
    if matches!(function.asyncness, ir::Asyncness::Async) {
        return Err(RenderError::new(format!(
            "`{}` is async; async functions land in phase 2 (issue #1992)",
            function.name
        )));
    }
    let wrapper = wrapper_ident(function);
    let rust_name = names::name_ident(&function.name)?;

    let mut params = Vec::new();
    let mut bindings = Vec::new();
    let mut forwarded = Vec::new();
    for arg in &function.args {
        let ident = names::name_ident(&arg.name)?;
        let bridge_ty = repr::bridge_type(&arg.ty);
        params.push(quote!(#ident: #bridge_ty));
        let ArgPassing { binding, call } = arg_passing(&arg.ty, &ident);
        bindings.extend(binding);
        forwarded.push(call);
    }

    let call = quote!(super::#user::#rust_name(#(#forwarded),*));
    // A throwing Ok type swift-bridge cannot name in a `Result` rides an
    // opaque carrier box instead (see `repr::throws_ok_box`).
    let ok_box = if function.throws.is_some() {
        function.ret.as_ref().and_then(repr::throws_ok_box)
    } else {
        None
    };
    let ok_bridge = ok_box.as_ref().map_or_else(
        || {
            function
                .ret
                .as_ref()
                .map_or_else(|| quote!(()), repr::bridge_type)
        },
        |shape| {
            let ident = shape.ident();
            quote!(#ident)
        },
    );
    let WrapperTail { ret, body } =
        wrapper_tail(function, interface, ffi_mod, &ok_bridge, ok_box.as_ref(), &call)?;

    let bridge_ret = if function.throws.is_some() || function.ret.is_some() {
        let bare = bridge_return(function, interface, &ok_bridge)?;
        quote!(-> #bare)
    } else {
        TokenStream::new()
    };
    let bridge_decl = quote! {
        fn #wrapper(#(#params),*) #bridge_ret;
    };
    let item = quote! {
        fn #wrapper(#(#params),*) -> #ret {
            #(#bindings)*
            #body
        }
    };
    Ok(RenderedFunction {
        bridge_decl,
        item,
    })
}

/// The bare return type inside the bridge module (textual grammar: bare
/// `Result`, no `::std::` paths).
fn bridge_return(
    function: &ir::Function,
    interface: &ir::Interface,
    ok_bridge: &TokenStream,
) -> Result<TokenStream, RenderError> {
    let Some(throws) = &function.throws else {
        return Ok(ok_bridge.clone());
    };
    let err = bridge_error_ident(interface, throws)?;
    Ok(quote!(Result<#ok_bridge, #err>))
}

/// The wrapper's return type and body, which vary together on `throws`.
struct WrapperTail {
    ret: TokenStream,
    body: TokenStream,
}

fn wrapper_tail(
    function: &ir::Function,
    interface: &ir::Interface,
    ffi_mod: &Ident,
    ok_bridge: &TokenStream,
    ok_box: Option<&repr::BoxShape>,
    call: &TokenStream,
) -> Result<WrapperTail, RenderError> {
    let ok_value = quote!(value);
    let ok_converted = ok_box.map_or_else(
        || {
            function
                .ret
                .as_ref()
                .map_or_else(|| quote!(value), |ret| repr::to_repr(ret, &ok_value))
        },
        |shape| {
            let ident = shape.ident();
            quote!(#ident::from_value(value))
        },
    );
    let Some(throws) = &function.throws else {
        let body = function
            .ret
            .as_ref()
            .map_or_else(|| call.clone(), |ret| repr::to_repr(ret, call));
        return Ok(WrapperTail {
            ret: ok_bridge.clone(),
            body,
        });
    };
    let err = bridge_error_ident(interface, throws)?;
    Ok(WrapperTail {
        ret: quote!(::std::result::Result<#ok_bridge, #ffi_mod::#err>),
        body: quote! {
            match #call {
                ::std::result::Result::Ok(value) => ::std::result::Result::Ok(#ok_converted),
                ::std::result::Result::Err(error) => {
                    ::std::result::Result::Err(::std::convert::From::from(error))
                }
            }
        },
    })
}

fn bridge_error_ident(interface: &ir::Interface, throws: &str) -> Result<Ident, RenderError> {
    interface
        .errors
        .iter()
        .find(|error| error.name == throws)
        .map(error::bridge_ident)
        .ok_or_else(|| {
            RenderError::new(format!(
                "`{throws}` is not a #[unibind::error] in this module"
            ))
        })
}

/// How one argument travels from the wrapper signature to the user call:
/// an optional rebinding statement plus the call-site expression.
struct ArgPassing {
    binding: Option<TokenStream>,
    call: TokenStream,
}

fn arg_passing(ty: &ir::Type, ident: &Ident) -> ArgPassing {
    let expr = quote!(#ident);
    match ty {
        ir::Type::String { owned: false } => ArgPassing {
            binding: None,
            call: quote!(#ident.as_str()),
        },
        ir::Type::Bytes { owned: false } => ArgPassing {
            binding: None,
            call: quote!(#ident.as_slice()),
        },
        ir::Type::Path { owned: false } => ArgPassing {
            binding: Some(quote! {
                let #ident = ::std::path::PathBuf::from(#ident);
            }),
            call: quote!(#ident.as_path()),
        },
        ir::Type::Option(inner) => match &**inner {
            ir::Type::String { owned: false } => ArgPassing {
                binding: None,
                call: quote!(#ident.as_deref()),
            },
            ir::Type::Path { owned: false } => ArgPassing {
                binding: Some(quote! {
                    let #ident = #ident.map(::std::path::PathBuf::from);
                }),
                call: quote!(#ident.as_deref()),
            },
            // `Option<&[u8]>`: the box hands back an owned `Option<Vec<u8>>`;
            // rebind it so the call can borrow.
            ir::Type::Bytes { owned: false } => ArgPassing {
                binding: Some(quote! {
                    let #ident = #ident.into_value();
                }),
                call: quote!(#ident.as_deref()),
            },
            _ => ArgPassing {
                binding: None,
                call: repr::from_repr(ty, &expr),
            },
        },
        _ => ArgPassing {
            binding: None,
            call: repr::from_repr(ty, &expr),
        },
    }
}
