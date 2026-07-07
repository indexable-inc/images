//! Render the generated client's `src/engine.rs`: the `Engine` handle that
//! dlopens the cdylib, runs the IR-hash handshake, resolves every export
//! through stabby's report check, and wraps each one idiomatically.

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::record::fallback_docs;
use crate::ty::{self, Paths};
use crate::{function, module, RenderError};

/// The full `src/engine.rs` token stream.
pub(crate) fn engine_file(interface: &ir::Interface) -> Result<TokenStream, RenderError> {
    let paths = client_paths(interface);
    let hash = module::ir_sha256_hex(interface)?;
    let handshake_symbol = function::ir_sha256_symbol(interface);
    let handshake_bytes = Literal::byte_string(handshake_symbol.as_bytes());

    let fields = interface.functions.iter().map(|func| {
        let ident = ty::name_ident(&func.name);
        let signature = function::signature_type(func, &paths);
        quote!(#ident: #signature,)
    });
    let resolutions = interface.functions.iter().map(|func| {
        let ident = ty::name_ident(&func.name);
        let signature = function::signature_type(func, &paths);
        let symbol = function::symbol(interface, &func.name);
        let symbol_bytes = Literal::byte_string(symbol.as_bytes());
        quote! {
            // SAFETY: `get_stabbied` validates the symbol's type report
            // before handing out the pointer.
            let #ident = *unsafe { library.get_stabbied::<#signature>(#symbol_bytes) }
                .map_err(symbol_error(#symbol))?;
        }
    });
    let field_names = interface.functions.iter().map(|func| ty::name_ident(&func.name));
    let methods = interface
        .functions
        .iter()
        .map(|func| method(func, &paths));
    let wrappers = interface
        .functions
        .iter()
        .filter(|func| matches!(func.asyncness, ir::Asyncness::Async))
        .map(|func| future_wrapper(func, &paths));
    let stream_wrappers = interface
        .functions
        .iter()
        .filter(|func| function::returns_stream(func))
        .map(|func| stream_wrapper(func, &paths));
    let module_doc = format!(
        "The safe handle over the `{}` engine cdylib: loading, the IR-hash \
         handshake, and one method per exported function.",
        interface.name
    );

    Ok(quote! {
        #![doc = #module_doc]

        use ::stabby::libloading::StabbyLibrary as _;

        use crate::error::LoadError;

        /// Hex SHA-256 of the interface IR this client was generated from,
        /// compared against the engine's handshake symbol at load time.
        const EXPECTED_IR_SHA256: &str = #hash;

        /// A loaded engine with every export resolved and typed.
        ///
        /// The `Engine` keeps the library mapped for its whole lifetime and
        /// never exposes unloading: values and futures returned by its
        /// methods point into the library's code.
        pub struct Engine {
            /// Keeps the library mapped; the resolved pointers below point
            /// into it.
            _library: ::libloading::Library,
            #(#fields)*
        }

        impl Engine {
            /// Load the engine cdylib at `path` and resolve every export.
            ///
            /// The IR-hash handshake runs first: the engine must report
            /// exactly the interface hash this client was generated from.
            /// Nothing loads on a mismatch; there is no fallback.
            ///
            /// # Errors
            ///
            /// [`LoadError`] when the library cannot be opened, a symbol is
            /// missing or fails stabby's structural report check, or the
            /// IR hashes disagree.
            pub fn load(path: &::std::path::Path) -> ::std::result::Result<Self, LoadError> {
                // SAFETY: dlopen runs the library's initializers; the engine
                // is the generated counterpart of this client.
                let library = unsafe { ::libloading::Library::new(path) }.map_err(|error| {
                    LoadError::Dlopen {
                        message: ::std::string::ToString::to_string(&error),
                    }
                })?;
                // SAFETY: `get_stabbied` validates the symbol's type report
                // before handing out the pointer.
                let ir_sha256 = *unsafe {
                    library.get_stabbied::<extern "C" fn() -> ::stabby::str::Str<'static>>(
                        #handshake_bytes,
                    )
                }
                .map_err(symbol_error(#handshake_symbol))?;
                let actual: &'static str = ::core::convert::Into::into(ir_sha256());
                if actual != EXPECTED_IR_SHA256 {
                    return ::std::result::Result::Err(LoadError::IrHashMismatch {
                        expected: EXPECTED_IR_SHA256.to_owned(),
                        actual: actual.to_owned(),
                    });
                }
                #(#resolutions)*
                ::std::result::Result::Ok(Self {
                    _library: library,
                    #(#field_names,)*
                })
            }

            #(#methods)*
        }

        #(#wrappers)*

        #(#stream_wrappers)*

        /// Classify a `get_stabbied` failure: a loader error means the
        /// symbol is missing, anything else is stabby's type-report
        /// mismatch text.
        fn symbol_error(
            symbol: &'static str,
        ) -> impl Fn(::std::boxed::Box<dyn ::std::error::Error + Send + Sync>) -> LoadError {
            move |error| {
                let message = ::std::string::ToString::to_string(&error);
                if error.is::<::libloading::Error>() {
                    LoadError::MissingSymbol {
                        symbol: symbol.to_owned(),
                        message,
                    }
                } else {
                    LoadError::SignatureMismatch {
                        symbol: symbol.to_owned(),
                        message,
                    }
                }
            }
        }
    })
}

/// The `Paths` every client file shares: idiomatic records under
/// `crate::records`, ABI mirrors under `crate::abi`.
pub(crate) fn client_paths(interface: &ir::Interface) -> Paths {
    Paths {
        plain: quote!(crate::records::),
        mirror: quote!(crate::abi::),
        report_module: module::report_module(interface),
    }
}

/// One safe method wrapping one export.
fn method(func: &ir::Function, paths: &Paths) -> TokenStream {
    let ident = ty::name_ident(&func.name);
    let docs = fallback_docs(&func.docs, &format!("Call the engine's `{}`.", func.name));

    let mut params = Vec::new();
    let mut conversions = Vec::new();
    let mut call_args = Vec::new();
    for arg in &func.args {
        let arg_ident = ty::name_ident(&arg.name);
        let plain = ty::plain_type(&arg.ty, paths);
        params.push(quote!(#arg_ident: #plain));
        // Identity-typed arguments pass straight through; a rebinding
        // `let x: T = x;` would trip `clippy::redundant_locals`.
        if !ty::is_identity(&arg.ty) {
            let stable = ty::stable_type(&arg.ty, paths);
            let converted = ty::to_stable(&quote!(#arg_ident), &arg.ty, paths);
            conversions.push(quote!(let #arg_ident: #stable = #converted;));
        }
        call_args.push(quote!(#arg_ident));
    }
    let call = quote!((self.#ident)(#(#call_args),*));

    if function::returns_stream(func) {
        let wrapper = function::stream_wrapper_ident(func);
        let wrapper_doc = format!(
            "The returned [`{wrapper}`] yields the items; dropping it \
             before the end drops the engine-side stream."
        );
        return quote! {
            #docs
            ///
            #[doc = #wrapper_doc]
            pub fn #ident(&self, #(#params),*) -> #wrapper {
                #(#conversions)*
                #wrapper { inner: #call }
            }
        };
    }
    match func.asyncness {
        ir::Asyncness::Sync => sync_method(func, paths, &SyncMethodParts {
            ident,
            docs,
            params,
            conversions,
            call,
        }),
        ir::Asyncness::Async => {
            let wrapper = function::future_wrapper_ident(func);
            let wrapper_doc = format!(
                "The returned [`{wrapper}`] resolves the call; dropping it \
                 before completion cancels the engine-side future."
            );
            quote! {
                #docs
                ///
                #[doc = #wrapper_doc]
                pub fn #ident(&self, #(#params),*) -> #wrapper {
                    #(#conversions)*
                    #wrapper { inner: #call }
                }
            }
        }
    }
}

/// The pieces a sync method is assembled from.
struct SyncMethodParts {
    ident: proc_macro2::Ident,
    docs: TokenStream,
    params: Vec<TokenStream>,
    conversions: Vec<TokenStream>,
    call: TokenStream,
}

fn sync_method(func: &ir::Function, paths: &Paths, parts: &SyncMethodParts) -> TokenStream {
    let SyncMethodParts {
        ident,
        docs,
        params,
        conversions,
        call,
    } = parts;
    match (&func.throws, &func.ret) {
        (None, None) => quote! {
            #docs
            pub fn #ident(&self, #(#params),*) {
                #(#conversions)*
                #call;
            }
        },
        (None, Some(ret)) => {
            let plain = ty::owned_plain_type(ret, paths);
            // Identity results return the call directly; a
            // `let out = call; out` tail would trip `clippy::let_and_return`.
            let body = if ty::is_identity(ret) {
                call.clone()
            } else {
                let converted = ty::to_plain(&quote!(out), ret, paths);
                quote! {
                    let out = #call;
                    #converted
                }
            };
            quote! {
                #docs
                #[must_use]
                pub fn #ident(&self, #(#params),*) -> #plain {
                    #(#conversions)*
                    #body
                }
            }
        }
        (Some(error), ret) => {
            let error_ident = ty::name_ident(error);
            let ok = ret
                .as_ref()
                .map_or_else(|| quote!(()), |ret| ty::owned_plain_type(ret, paths));
            let ok_value = ret.as_ref().map_or_else(
                || quote!(::std::result::Result::Ok(())),
                |ret| {
                    let converted = ty::to_plain(&quote!(out), ret, paths);
                    quote!(::std::result::Result::Ok(#converted))
                },
            );
            let errors_doc = format!(
                "Returns the engine's [`{error}`](crate::error::{error}) when the call fails."
            );
            // The user's own docs may already carry an `# Errors` section
            // (rustdoc would render two headings); only add ours when the
            // IR docs have none.
            let errors_section = (!func.docs.iter().any(|line| line.contains("# Errors")))
                .then(|| {
                    quote! {
                        ///
                        /// # Errors
                        ///
                        #[doc = #errors_doc]
                    }
                });
            quote! {
                #docs
                #errors_section
                pub fn #ident(
                    &self,
                    #(#params),*
                ) -> ::std::result::Result<#ok, crate::error::#error_ident> {
                    #(#conversions)*
                    match ::std::result::Result::from(#call) {
                        ::std::result::Result::Ok(out) => #ok_value,
                        ::std::result::Result::Err(error) => ::std::result::Result::Err(
                            crate::error::#error_ident::from(error),
                        ),
                    }
                }
            }
        }
    }
}

/// The named stream wrapper for one stream-returning export. Same shape as
/// the future wrappers: a thin struct whose `Drop` drops the boxed engine
/// stream through the ABI vtable.
fn stream_wrapper(func: &ir::Function, paths: &Paths) -> TokenStream {
    let wrapper = function::stream_wrapper_ident(func);
    let ident = ty::name_ident(&func.name);
    let Some(ir::Type::Stream(item)) = &func.ret else {
        unreachable!("stream wrappers only render for stream returns");
    };
    let stable_item = ty::stable_type(item, paths);
    let plain_item = ty::owned_plain_type(item, paths);
    let doc = format!(
        "Stream returned by [`Engine::{ident}`]. Dropping it before the end \
         drops the engine-side stream through the ABI vtable, cancelling it \
         inside the engine."
    );
    // `to_plain` is the identity for primitive items, so this covers both.
    let converted = ty::to_plain(&quote!(out), item, paths);
    let ready_some =
        quote!(::core::task::Poll::Ready(::std::option::Option::Some(#converted)));
    quote! {
        #[doc = #doc]
        #[must_use = "streams do nothing unless polled"]
        pub struct #wrapper {
            inner: ::unibind_stream::DynStream<'static, #stable_item>,
        }

        impl ::futures_core::Stream for #wrapper {
            type Item = #plain_item;

            fn poll_next(
                self: ::core::pin::Pin<&mut Self>,
                context: &mut ::core::task::Context<'_>,
            ) -> ::core::task::Poll<::std::option::Option<Self::Item>> {
                // The `Dyn` box is `Unpin` (a pointer plus a static
                // vtable), so projecting through `get_mut` is sound.
                let this = self.get_mut();
                match ::unibind_stream::poll_next(&mut this.inner, context) {
                    ::core::task::Poll::Ready(::std::option::Option::Some(out)) => #ready_some,
                    ::core::task::Poll::Ready(::std::option::Option::None) => {
                        ::core::task::Poll::Ready(::std::option::Option::None)
                    }
                    ::core::task::Poll::Pending => ::core::task::Poll::Pending,
                }
            }
        }
    }
}

/// The named future wrapper for one async export. A thin struct keeps the
/// `Drop` semantics obvious: dropping it drops the boxed engine future
/// through the ABI vtable, which cancels it inside the engine.
fn future_wrapper(func: &ir::Function, paths: &Paths) -> TokenStream {
    let wrapper = function::future_wrapper_ident(func);
    let ident = ty::name_ident(&func.name);
    let inner = function::stable_return(func, paths)
        .unwrap_or_else(|| quote!(::stabby::future::DynFuture<'static, ()>));
    let doc = format!(
        "Future returned by [`Engine::{ident}`]. Dropping it before \
         completion drops the engine-side future through the ABI vtable, \
         cancelling it inside the engine."
    );

    let (output, converted) = match (&func.throws, &func.ret) {
        (None, None) => (quote!(()), quote!(out)),
        (None, Some(ret)) => (
            ty::owned_plain_type(ret, paths),
            ty::to_plain(&quote!(out), ret, paths),
        ),
        (Some(error), ret) => {
            let error_ident = ty::name_ident(error);
            let ok = ret
                .as_ref()
                .map_or_else(|| quote!(()), |ret| ty::owned_plain_type(ret, paths));
            let ok_value = ret.as_ref().map_or_else(
                || quote!(::std::result::Result::Ok(())),
                |ret| {
                    let converted = ty::to_plain(&quote!(out), ret, paths);
                    quote!(::std::result::Result::Ok(#converted))
                },
            );
            (
                quote!(::std::result::Result<#ok, crate::error::#error_ident>),
                quote! {
                    match ::std::result::Result::from(out) {
                        ::std::result::Result::Ok(out) => #ok_value,
                        ::std::result::Result::Err(error) => ::std::result::Result::Err(
                            crate::error::#error_ident::from(error),
                        ),
                    }
                },
            )
        }
    };

    quote! {
        #[doc = #doc]
        #[must_use = "futures do nothing unless polled"]
        pub struct #wrapper {
            inner: #inner,
        }

        impl ::core::future::Future for #wrapper {
            type Output = #output;

            fn poll(
                self: ::core::pin::Pin<&mut Self>,
                context: &mut ::core::task::Context<'_>,
            ) -> ::core::task::Poll<Self::Output> {
                // `DynFuture` is `Unpin` (a pointer plus a static vtable),
                // so projecting through `get_mut` is sound.
                let this = self.get_mut();
                match ::core::pin::Pin::new(&mut this.inner).poll(context) {
                    ::core::task::Poll::Ready(out) => ::core::task::Poll::Ready(#converted),
                    ::core::task::Poll::Pending => ::core::task::Poll::Pending,
                }
            }
        }
    }
}
