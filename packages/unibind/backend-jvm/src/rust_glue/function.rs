//! Render one `extern "C"` export, its `__free` companion, and the shared
//! panic plumbing.
//!
//! Every export wraps its body in `catch_unwind`, so unwinding never
//! crosses the C boundary: a panic becomes an envelope with `code == -1`
//! carrying the payload text.

use proc_macro2::{Ident, Literal, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::ctype::CTy;
use crate::model::Model;
use crate::rust_glue::{decode, encode, types};
use crate::{names, RenderError};

/// The shared panic-payload helper.
pub fn helpers() -> TokenStream {
    quote! {
        /// Best-effort text from a caught panic payload.
        fn panic_text(payload: &(dyn ::std::any::Any + ::core::marker::Send)) -> ::std::string::String {
            if let ::std::option::Option::Some(text) = payload.downcast_ref::<&str>() {
                return ::std::borrow::ToOwned::to_owned(*text);
            }
            if let ::std::option::Option::Some(text) = payload.downcast_ref::<::std::string::String>() {
                return ::std::clone::Clone::clone(text);
            }
            ::std::borrow::ToOwned::to_owned("panic across the unibind boundary")
        }
    }
}

/// The ABI version probe.
pub fn abi_version(module: &str) -> TokenStream {
    let ident = format_ident!("{}", names::abi_symbol(module));
    quote! {
        /// ABI revision of these exports; the Java binding checks it at
        /// load.
        #[unsafe(no_mangle)]
        pub extern "C" fn #ident() -> u32 {
            0
        }
    }
}

/// One function's export and free pair.
pub fn render_fn(
    function: &ir::Function,
    interface: &ir::Interface,
    model: &Model<'_>,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    let export_ident = format_ident!("{}", names::export_symbol(&interface.name, &function.name));
    let free_ident = format_ident!("{}", names::free_symbol(&interface.name, &function.name));
    let envelope = types::envelope_ident(&function.name);

    let mut params = Vec::new();
    let mut bindings = Vec::new();
    let mut forwarded = Vec::new();
    for arg in &function.args {
        let ident = names::rust_ident(&arg.name)?;
        let cty = CTy::of(&arg.ty);
        let mirror = types::mirror_tokens(&cty);
        if cty.is_scalar() {
            params.push(quote!(#ident: #mirror));
            if matches!(arg.ty, ir::Type::Bool) {
                bindings.push(quote!(let #ident = #ident != 0;));
            }
        } else {
            params.push(quote!(#ident: *const #mirror));
            let decoded = decode::expr(model, &arg.ty, &quote!(#ident), user)?;
            bindings.push(quote! {
                let #ident = unsafe { &*#ident };
                let #ident = #decoded;
            });
        }
        forwarded.push(quote!(#ident));
    }

    let rust_name = names::rust_ident(&function.name)?;
    let call = quote!(super::#user::#rust_name(#(#forwarded),*));
    let ok_value = match &function.ret {
        Some(ret) => {
            let encoded = encode::expr(model, ret, &quote!(value))?;
            Some(quote!(value: #encoded,))
        }
        None => None,
    };
    let zero_value = function
        .ret
        .as_ref()
        .map(|_| quote!(value: unsafe { ::core::mem::zeroed() },));

    let body = match &function.throws {
        Some(throws) => {
            let error = interface
                .errors
                .iter()
                .find(|error| error.name == *throws)
                .expect("throws names are validated when the model is built");
            throws_body(ThrowsBody {
                function,
                error,
                call: &call,
                envelope: &envelope,
                ok_value: ok_value.as_ref(),
                zero_value: zero_value.as_ref(),
                user,
            })?
        }
        None => plain_body(function, &call, &envelope, ok_value.as_ref()),
    };

    let docs = &function.docs;
    Ok(quote! {
        #(#[doc = #docs])*
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #export_ident(#(#params),*) -> *mut #envelope {
            let outcome = ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                #(#bindings)*
                #body
            }));
            let envelope = match outcome {
                ::std::result::Result::Ok(envelope) => envelope,
                ::std::result::Result::Err(payload) => #envelope {
                    code: -1,
                    err_msg: string_value(panic_text(payload.as_ref())),
                    #zero_value
                },
            };
            ::std::boxed::Box::into_raw(::std::boxed::Box::new(envelope))
        }

        /// Reclaim an envelope returned by the paired export. Null is a
        /// no-op; anything else must come from that export, exactly once.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #free_ident(envelope: *mut #envelope) {
            if envelope.is_null() {
                return;
            }
            drop(unsafe { ::std::boxed::Box::from_raw(envelope) });
        }
    })
}

fn plain_body(
    function: &ir::Function,
    call: &TokenStream,
    envelope: &Ident,
    ok_value: Option<&TokenStream>,
) -> TokenStream {
    if function.ret.is_some() {
        quote! {
            let value = #call;
            #envelope {
                code: 0,
                err_msg: null_string(),
                #ok_value
            }
        }
    } else {
        quote! {
            #call;
            #envelope {
                code: 0,
                err_msg: null_string(),
            }
        }
    }
}

/// Everything a `throws` function's match body needs.
#[derive(Clone, Copy)]
struct ThrowsBody<'a> {
    function: &'a ir::Function,
    error: &'a ir::ErrorType,
    call: &'a TokenStream,
    envelope: &'a Ident,
    ok_value: Option<&'a TokenStream>,
    zero_value: Option<&'a TokenStream>,
    user: &'a Ident,
}

fn throws_body(parts: ThrowsBody<'_>) -> Result<TokenStream, RenderError> {
    let ThrowsBody {
        function,
        error,
        call,
        envelope,
        ok_value,
        zero_value,
        user,
    } = parts;
    let error_ident = names::rust_ident(&error.name)?;
    let mut arms = Vec::new();
    for (index, variant) in error.variants.iter().enumerate() {
        let variant_ident = names::rust_ident(&variant.name)?;
        let code = Literal::usize_unsuffixed(index + 1);
        arms.push(quote! {
            super::#user::#error_ident::#variant_ident { .. } => #code,
        });
    }
    let ok_pattern = if function.ret.is_some() {
        quote!(value)
    } else {
        quote!(())
    };
    Ok(quote! {
        match #call {
            ::std::result::Result::Ok(#ok_pattern) => #envelope {
                code: 0,
                err_msg: null_string(),
                #ok_value
            },
            ::std::result::Result::Err(error) => #envelope {
                code: match &error {
                    #(#arms)*
                },
                err_msg: string_value(::std::string::ToString::to_string(&error)),
                #zero_value
            },
        }
    })
}
