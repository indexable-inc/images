//! Per-function rendering: export symbols, boundary signatures, and the
//! `#[stabby::export]` engine wrappers.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::ty::{self, Paths};

/// The exported symbol for one function: `unibind_<module>_<fn>`. The
/// interface name is used as-is (leading underscores and all), so the symbol
/// space follows the exported module verbatim.
pub fn symbol(interface: &ir::Interface, name: &str) -> String {
    format!("unibind_{}_{name}", interface.name)
}

/// The handshake symbol carrying the hex SHA-256 of the IR JSON bytes.
pub fn ir_sha256_symbol(interface: &ir::Interface) -> String {
    symbol(interface, "ir_sha256")
}

/// The stable return type a function's export carries: the plain stable
/// type, wrapped in `stabby::result::Result` when the function throws and in
/// `DynFuture` when it suspends. `None` means the export returns unit.
pub fn stable_return(function: &ir::Function, paths: &Paths) -> Option<TokenStream> {
    let ok = function
        .ret
        .as_ref()
        .map(|ret| ty::stable_type(ret, paths));
    let sync = match (&function.throws, ok) {
        (None, ok) => ok,
        (Some(error), ok) => {
            let ok = ok.unwrap_or_else(|| quote!(()));
            let error = ty::name_ident(&format!("{error}Stable"));
            let mirror = &paths.mirror;
            Some(quote!(::stabby::result::Result<#ok, #mirror #error>))
        }
    };
    match function.asyncness {
        ir::Asyncness::Sync => sync,
        ir::Asyncness::Async => {
            let output = sync.unwrap_or_else(|| quote!(()));
            Some(quote!(::stabby::future::DynFuture<'static, #output>))
        }
    }
}

/// The `extern "C"` fn-pointer type the client resolves for one export;
/// stabby's report check compares this type structurally against the
/// engine's.
pub fn signature_type(function: &ir::Function, paths: &Paths) -> TokenStream {
    let args = function
        .args
        .iter()
        .map(|arg| ty::stable_type(&arg.ty, paths));
    let ret = stable_return(function, paths).map(|ret| quote!(-> #ret));
    quote!(extern "C" fn(#(#args),*) #ret)
}

/// One `#[stabby::export]` wrapper: stable args in, plain call, stable value
/// out (boxed into a `DynFuture` for `async fn`).
pub fn render_export(
    interface: &ir::Interface,
    function: &ir::Function,
    paths: &Paths,
) -> TokenStream {
    let symbol = ty::name_ident(&symbol(interface, &function.name));
    let user_fn = ty::name_ident(&function.name);
    let plain = &paths.plain;

    let mut params = Vec::new();
    let mut conversions = Vec::new();
    let mut call_args = Vec::new();
    for arg in &function.args {
        let ident = ty::name_ident(&arg.name);
        let stable = ty::stable_type(&arg.ty, paths);
        params.push(quote!(#ident: #stable));
        let owned = ty::owned_plain_type(&arg.ty, paths);
        let converted = ty::to_plain(&quote!(#ident), &arg.ty, paths);
        conversions.push(quote!(let #ident: #owned = #converted;));
        call_args.push(call_arg(&arg.ty, &ident));
    }

    let call = quote!(#plain #user_fn(#(#call_args),*));
    let produce = render_result(function, &call, paths);
    let ret = stable_return(function, paths).map(|ret| quote!(-> #ret));
    let body = match function.asyncness {
        ir::Asyncness::Sync => quote! {
            #(#conversions)*
            #produce
        },
        // Conversions run before the async block so the future owns plain
        // values and stays `'static`; the user's future must be
        // `Send + Sync` (the `DynFuture` bound).
        ir::Asyncness::Async => quote! {
            #(#conversions)*
            ::stabby::boxed::Box::new(async move { #produce }).into()
        },
    };
    quote! {
        #[::stabby::export]
        pub extern "C" fn #symbol(#(#params),*) #ret {
            #body
        }
    }
}

/// The call expression plus result conversion; for `async fn` the call is
/// awaited inside the wrapper's async block.
fn render_result(function: &ir::Function, call: &TokenStream, paths: &Paths) -> TokenStream {
    let call = match function.asyncness {
        ir::Asyncness::Sync => call.clone(),
        ir::Asyncness::Async => quote!(#call.await),
    };
    match (&function.throws, &function.ret) {
        (None, None) => call,
        // A stream return boxes the user's `impl Stream` into the shared
        // dynptr shape through `unibind-stream`'s adapter (which tracks
        // termination for the two-call raw protocol).
        (None, Some(ir::Type::Stream(_))) => quote! {
            ::stabby::boxed::Box::new(::unibind_stream::StreamAdapter::new(#call)).into()
        },
        (None, Some(ret)) => {
            let plain = ty::owned_plain_type(ret, paths);
            let converted = ty::to_stable(&quote!(out), ret, paths);
            quote! {
                let out: #plain = #call;
                #converted
            }
        }
        (Some(error), ret) => {
            let error = ty::name_ident(&format!("{error}Stable"));
            let mirror = &paths.mirror;
            let ok = ret.as_ref().map_or_else(
                || quote!(::stabby::result::Result::Ok(())),
                |ret| {
                    let converted = ty::to_stable(&quote!(out), ret, paths);
                    quote!(::stabby::result::Result::Ok(#converted))
                },
            );
            quote! {
                match #call {
                    ::std::result::Result::Ok(out) => #ok,
                    ::std::result::Result::Err(error) => {
                        ::stabby::result::Result::Err(#mirror #error::from(error))
                    }
                }
            }
        }
    }
}

/// How a converted, owned local is handed to a plain function: borrowed
/// types re-borrow, `Option`s of borrowed types go through `as_deref`.
fn call_arg(ty: &ir::Type, ident: &proc_macro2::Ident) -> TokenStream {
    if ty::is_borrowed(ty) {
        return quote!(&#ident);
    }
    if let ir::Type::Option(inner) = ty
        && ty::is_borrowed(inner)
    {
        return quote!(#ident.as_deref());
    }
    quote!(#ident)
}

/// A future-wrapper type name for one async function: `delayed_double` ->
/// `DelayedDoubleFuture`.
pub fn future_wrapper_ident(function: &ir::Function) -> proc_macro2::Ident {
    format_ident!("{}Future", pascal_case(&function.name))
}

/// A stream-wrapper type name for one stream-returning function:
/// `count_to` -> `CountToStream`.
pub fn stream_wrapper_ident(function: &ir::Function) -> proc_macro2::Ident {
    format_ident!("{}Stream", pascal_case(&function.name))
}

/// Whether the function's return is a stream (only ever at top level).
pub const fn returns_stream(function: &ir::Function) -> bool {
    matches!(function.ret, Some(ir::Type::Stream(_)))
}

fn pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect()
}

/// Doc attributes from IR doc lines.
pub fn doc_attrs(lines: &[String]) -> TokenStream {
    quote! { #(#[doc = #lines])* }
}
