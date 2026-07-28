//! Render the uniform `extern "C"` shim wrapping each exported function.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

use crate::{error, names, ty};

/// Render one function's shim: decode the arguments in declaration order,
/// call the user function, and encode the outcome through the runtime's
/// reply envelope.
pub fn render_fn(
    function: &ir::Function,
    interface: &ir::Interface,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    if matches!(function.asyncness, ir::Asyncness::Async) {
        return Err(RenderError::new(format!(
            "`{}` is an async fn, which the jvm backend does not drive; \
             export a sync fn that blocks on a runtime (the JVM caller can \
             move the call to a virtual thread)",
            function.name
        )));
    }
    if function.name == "free" {
        return Err(RenderError::new(
            "`free` collides with the interface's buffer-free symbol; rename \
             the function (a `jvm(name = ...)` rename does not move the \
             `extern \"C\"` symbol)",
        ));
    }
    // Validate the Java side alongside the glue: one validator for both.
    names::method_name(function)?;

    let mut decodes = Vec::new();
    let mut forwarded = Vec::new();
    for arg in &function.args {
        names::arg_name(arg)?;
        ty::check_boundary(&arg.ty, interface).map_err(|error| {
            RenderError::new(format!("argument `{}`: {}", arg.name, error.message))
        })?;
        let ident = name_ident(&arg.name)?;
        let decode = ty::decode_expr(&arg.ty, 0);
        decodes.push(quote!(let #ident = #decode;));
        forwarded.push(quote!(#ident));
    }

    if let Some(ret) = &function.ret {
        ty::check_boundary(ret, interface).map_err(|error| {
            RenderError::new(format!("return of `{}`: {}", function.name, error.message))
        })?;
    }

    let rust_name = name_ident(&function.name)?;
    let symbol = format_ident!("{}", names::symbol(interface, function));
    let call = quote!(super::#user::#rust_name(#(#forwarded),*));
    let outcome = function.throws.as_ref().map_or_else(
        || quote!(let __ret = #call;),
        |throws| {
            let fail = error::fail_ident(throws);
            quote!(let __ret = #call.map_err(#fail)?;)
        },
    );
    let encode = function.ret.as_ref().map_or_else(
        || quote!(let () = __ret;),
        |ret| ty::encode_stmts(ret, &quote!(__ret), 0),
    );

    let docs = format!(
        "C-ABI shim for `{}`; called only by the generated Java.",
        function.name
    );
    Ok(quote! {
        #[doc = #docs]
        #[unsafe(no_mangle)]
        unsafe extern "C" fn #symbol(
            args: *const u8,
            len: usize,
            out: *mut ::unibind_jvm_runtime::RawBuf,
        ) {
            unsafe {
                ::unibind_jvm_runtime::invoke(args, len, out, |reader, writer| {
                    #(#decodes)*
                    reader.finish();
                    #outcome
                    #encode
                    ::std::result::Result::Ok(())
                });
            }
        }
    })
}

/// The per-interface free shim reclaiming reply buffers.
pub fn render_free(interface: &ir::Interface) -> TokenStream {
    let symbol = format_ident!("{}", names::free_symbol(interface));
    quote! {
        /// Reclaim a reply buffer previously handed to Java.
        #[unsafe(no_mangle)]
        unsafe extern "C" fn #symbol(ptr: *mut u8, len: usize, cap: usize) {
            unsafe { ::unibind_jvm_runtime::free(ptr, len, cap) }
        }
    }
}
