//! Error mapping: a declared error crosses as its variant's declaration
//! index plus its `Display` text, and the generated Java rebuilds the
//! matching exception subclass from the index.

use heck::ToSnakeCase as _;
use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::{names, RenderError};

/// Java exception classes an error enum may extend via `jvm(base = ...)`.
/// The default is `RuntimeException`; the checked bases make the generated
/// methods' `throws` clauses mandatory rather than documentary.
pub const ALLOWED_BASES: &[&str] = &[
    "RuntimeException",
    "Exception",
    "IllegalArgumentException",
    "IllegalStateException",
    "IOException",
];

/// The glue-module function turning `error` into a wire
/// `unibind_jvm_runtime::Failure`.
pub fn fail_ident(error_name: &str) -> Ident {
    format_ident!("__fail_{}", error_name.to_snake_case())
}

/// Render the `__fail_<error>` mapper for one error enum, validating the
/// `jvm(base = ...)` choice both sides depend on.
pub fn render_error(error: &ir::ErrorType, user: &Ident) -> Result<TokenStream, RenderError> {
    if let Some(base) = &error.jvm_base
        && !ALLOWED_BASES.contains(&base.as_str())
    {
        return Err(RenderError::new(format!(
            "`{}` sets `jvm(base = \"{base}\")`, which is not a \
             supported Java base exception; pick one of {}",
            error.name,
            ALLOWED_BASES.join(", ")
        )));
    }
    // Validate the Java-side names alongside the glue.
    names::checked(names::exception_name(error), &format!("error `{}`", error.name))?;
    for variant in &error.variants {
        names::checked(
            names::variant_exception_name(variant),
            &format!("variant `{}` of error `{}`", variant.name, error.name),
        )?;
    }

    let rust_name = names::name_ident(&error.name)?;
    let fail = fail_ident(&error.name);
    let arms = error.variants.iter().enumerate().map(|(index, variant)| {
        let variant_ident = format_ident!("{}", &variant.name);
        let index = u32::try_from(index).expect("variant count fits u32");
        quote!(super::#user::#rust_name::#variant_ident { .. } => #index,)
    });

    let docs = format!(
        "Carry `{}` across the boundary: variant index plus `Display` text.",
        error.name
    );
    Ok(quote! {
        #[doc = #docs]
        fn #fail(error: super::#user::#rust_name) -> ::unibind_jvm_runtime::Failure {
            let message = ::std::string::ToString::to_string(&error);
            let variant = match &error {
                #(#arms)*
            };
            ::unibind_jvm_runtime::Failure { variant, message }
        }
    })
}
