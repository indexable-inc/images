//! Error rendering: the ABI-stable carrier struct shared by both sides, the
//! engine-side conversion from the user's enum, and the client's idiomatic
//! error enums (plus its `LoadError`).
//!
//! IR error variants carry only their `Display` text across the boundary
//! (same contract as the Python backend's exception hierarchy), so the
//! stable carrier is a variant index in declaration order plus one string.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;

use crate::record::fallback_docs;
use crate::ty::{self, Paths};

/// The stable carrier's type name for one error enum.
pub(crate) fn stable_ident(error: &ir::ErrorType) -> proc_macro2::Ident {
    ty::name_ident(&format!("{}Stable", error.name))
}

/// The carrier struct definition, emitted verbatim on both sides (like the
/// record mirrors). `no_opt` and the `module` override for the same reasons
/// as record mirrors: the layout is generated and report-checked, and the
/// report module must match across the two crates.
pub(crate) fn stable_struct(error: &ir::ErrorType, paths: &Paths) -> TokenStream {
    let name = stable_ident(error);
    let module = &paths.report_module;
    let doc = format!(
        "ABI-stable carrier for `{}`: variant index in declaration order plus \
         the variant's `Display` text.",
        error.name
    );
    quote! {
        #[doc = #doc]
        #[::stabby::stabby(no_opt, module = #module)]
        pub struct #name {
            /// Index of the variant, in declaration order.
            pub variant: u32,
            /// The variant's `Display` text.
            pub message: ::stabby::string::String,
        }
    }
}

/// Engine side: map the user's enum onto the carrier. Variants map to their
/// declaration index; the message is the enum's `Display` output, which the
/// user must implement (the same requirement the Python backend documents).
pub(crate) fn engine_conversion(error: &ir::ErrorType, paths: &Paths) -> TokenStream {
    let name = ty::name_ident(&error.name);
    let stable = stable_ident(error);
    let plain = &paths.plain;
    let arms = error.variants.iter().enumerate().map(|(index, variant)| {
        let variant = ty::name_ident(&variant.name);
        let index = u32::try_from(index).expect("fewer than 2^32 variants");
        quote!(#plain #name::#variant { .. } => #index,)
    });
    quote! {
        impl ::core::convert::From<#plain #name> for #stable {
            fn from(error: #plain #name) -> Self {
                let message =
                    ::stabby::string::String::from(::std::string::ToString::to_string(&error));
                let variant = match error {
                    #(#arms)*
                };
                Self { variant, message }
            }
        }
    }
}

/// Client side: the idiomatic error enum, one named-field variant per IR
/// variant, `Display` + `std::error::Error`, and the conversion from the
/// carrier.
pub(crate) fn client_error(error: &ir::ErrorType) -> TokenStream {
    let name = ty::name_ident(&error.name);
    let stable = stable_ident(error);
    let docs = fallback_docs(&error.docs, &format!("The `{}` error.", error.name));

    let variants = error.variants.iter().map(|variant| {
        let ident = ty::name_ident(&variant.name);
        let docs = fallback_docs(&variant.docs, &format!("The `{}` variant.", variant.name));
        quote! {
            #docs
            #ident {
                /// The engine-side variant's `Display` text.
                message: ::std::string::String,
            },
        }
    });
    let display_arms = error.variants.iter().map(|variant| {
        let ident = ty::name_ident(&variant.name);
        quote!(Self::#ident { message })
    });
    let from_arms = error.variants.iter().enumerate().map(|(index, variant)| {
        let ident = ty::name_ident(&variant.name);
        let index = u32::try_from(index).expect("fewer than 2^32 variants");
        quote!(#index => Self::#ident { message },)
    });
    quote! {
        #docs
        #[derive(Clone, Debug)]
        pub enum #name {
            #(#variants)*
        }

        impl ::core::fmt::Display for #name {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    #(#display_arms)|* => formatter.write_str(message),
                }
            }
        }

        impl ::std::error::Error for #name {}

        impl ::core::convert::From<crate::abi::#stable> for #name {
            fn from(raw: crate::abi::#stable) -> Self {
                let message = ::std::string::String::from(raw.message);
                match raw.variant {
                    #(#from_arms)*
                    other => unreachable!(
                        "the IR-hash handshake pins variant indices; got {other}"
                    ),
                }
            }
        }
    }
}

/// The client's load-time error: everything `Engine::load` can fail with.
/// Load never falls back: any mismatch is a hard error naming both sides.
pub(crate) fn load_error() -> TokenStream {
    quote! {
        /// Everything [`crate::Engine::load`] can fail with. Loading never
        /// falls back: a mismatch is a hard error naming both sides.
        #[derive(Debug)]
        pub enum LoadError {
            /// The engine library could not be opened.
            Dlopen {
                /// The loader's error text.
                message: ::std::string::String,
            },
            /// An expected `#[stabby::export]` symbol is missing; the engine
            /// was probably built without the `rs` unibind feature or with a
            /// different stabby major version.
            MissingSymbol {
                /// The symbol that failed to resolve.
                symbol: ::std::string::String,
                /// The loader's error text.
                message: ::std::string::String,
            },
            /// A symbol resolved, but stabby's structural type report does
            /// not match this client's expected signature.
            SignatureMismatch {
                /// The symbol whose report mismatched.
                symbol: ::std::string::String,
                /// Both type reports, as rendered by stabby.
                message: ::std::string::String,
            },
            /// The engine was generated from a different interface than this
            /// client: the IR hashes disagree.
            IrHashMismatch {
                /// The hex SHA-256 this client was generated from.
                expected: ::std::string::String,
                /// The hex SHA-256 the engine reported.
                actual: ::std::string::String,
            },
        }

        impl ::core::fmt::Display for LoadError {
            fn fmt(&self, formatter: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    Self::Dlopen { message } => {
                        write!(formatter, "opening the engine library failed: {message}")
                    }
                    Self::MissingSymbol { symbol, message } => {
                        write!(formatter, "symbol `{symbol}` did not resolve: {message}")
                    }
                    Self::SignatureMismatch { symbol, message } => {
                        write!(formatter, "symbol `{symbol}` has a mismatching ABI: {message}")
                    }
                    Self::IrHashMismatch { expected, actual } => {
                        write!(
                            formatter,
                            "engine/client interface mismatch: client was generated from IR \
                             {expected}, engine reports {actual}; regenerate the client"
                        )
                    }
                }
            }
        }

        impl ::std::error::Error for LoadError {}
    }
}
