//! Render the handle class behind a `UniStream`-returning function.
//!
//! Pull semantics: JavaScript calls `next()` and gets a promise of the next
//! element or `null` at the end, so nothing is produced faster than the
//! consumer asks (backpressure falls out of the pull). The pull and close
//! mechanics live in `unibind_runtime::PullStream`; the class here is the
//! thin napi shell that names the element type. The generated `index.js`
//! wraps the handle into a real `AsyncIterable`; the surface stays minimal
//! on purpose: `next` and `close`.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{Ownership, RenderError, pascal_case, rust_type};

use crate::function::{self, doc_attrs};
use crate::ty::{self, Level, TyCtx};

/// Render the wrapper function and the handle class for one
/// stream-returning function; the wrapper itself (sync or async, plain or
/// throwing) rides the shared callable path, which wraps the returned
/// `UniStream` into the class.
pub fn render_stream_fn(
    function: &ir::Function,
    element: &ir::Type,
    ctx: &TyCtx<'_>,
) -> Result<TokenStream, RenderError> {
    ty::check(
        element,
        &format!("the stream element of `{}`", function.name),
    )?;
    if let ir::Type::Named(name) = element
        && ctx.object(name).is_some()
    {
        return Err(RenderError::new(format!(
            "`{}` streams objects; streams carry data (records and \
             primitives) for now (issue #1993)",
            function.name
        )));
    }
    let class = ty::stream_class_ident(&function.name);
    let js_class = format!(
        "{}Stream",
        pascal_case(function.names.ts.as_deref().unwrap_or(&function.name))
    );
    // Storage spells the user's own element type, which is what the
    // producer yields; only the value handed to JavaScript picks up the
    // boundary shapes (`Buffer`, `BigInt`, a record's mirror).
    let element_decl = rust_type(element, ctx.user, Ownership::Owned);
    let element_top = ty::decl(element, ctx, Level::Top)?;
    let element_ret = ty::ret(element, ctx, &quote!(value));

    let wrapper_fn = function::render_fn(function, ctx)?;
    let class_docs = doc_attrs(&[format!(
        "Pull handle over the stream returned by `{}`.",
        function.name
    )]);
    let class_item = stream_class(
        &class,
        &js_class,
        &class_docs,
        &element_decl,
        &element_top,
        &element_ret,
    );
    Ok(quote! {
        #wrapper_fn
        #class_item
    })
}

/// The generated handle class: a napi shell over
/// `unibind_runtime::PullStream`, which owns the pull and close mechanics.
fn stream_class(
    class: &proc_macro2::Ident,
    js_class: &str,
    class_docs: &TokenStream,
    element_decl: &TokenStream,
    element_top: &TokenStream,
    element_ret: &TokenStream,
) -> TokenStream {
    quote! {
        #class_docs
        #[::napi_derive::napi(js_name = #js_class)]
        pub struct #class {
            stream: ::unibind_runtime::PullStream<#element_decl>,
        }

        impl #class {
            fn __unibind_from(stream: ::unibind_runtime::UniStream<#element_decl>) -> Self {
                Self {
                    stream: ::unibind_runtime::PullStream::new(stream),
                }
            }
        }

        #[::napi_derive::napi]
        impl #class {
            /// The next element, or `null` once the stream ends or closes.
            // napi's impl expansion only exports methods that carry their own
            // `#[napi]` attribute, so the marker below is load-bearing.
            #[::napi_derive::napi]
            pub async fn next(&self) -> ::std::option::Option<#element_top> {
                let value = self.stream.next().await?;
                ::std::option::Option::Some(#element_ret)
            }

            /// Drop the stream early; a pull in flight resolves `null`, and
            /// the producer sees its stream dropped.
            #[::napi_derive::napi]
            pub fn close(&self) {
                self.stream.close();
            }
        }
    }
}
