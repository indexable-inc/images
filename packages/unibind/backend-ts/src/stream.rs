//! Render the handle class behind a `UniStream`-returning function.
//!
//! Pull semantics: JavaScript calls `next()` and gets a promise of the next
//! element or `null` at the end, so nothing is produced faster than the
//! consumer asks (backpressure falls out of the pull). `close()` is
//! synchronous and race-free: a watch channel flips to closed, which both
//! wakes a pull blocked inside `next()` (dropping the checked-out stream)
//! and keeps later pulls from checking the stream back in. The generated
//! `index.js` wraps the handle into a real `AsyncIterable`; the surface
//! here stays minimal on purpose: `next` and `close`.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;

use crate::function::{self, doc_attrs};
use crate::ty::{self, Level, TyCtx};
use crate::RenderError;

/// Render the wrapper function and the handle class for one
/// stream-returning function; the wrapper itself (sync or async, plain or
/// throwing) rides the shared callable path, which wraps the returned
/// `UniStream` into the class.
pub fn render_stream_fn(
    function: &ir::Function,
    element: &ir::Type,
    ctx: &TyCtx<'_>,
) -> Result<TokenStream, RenderError> {
    ty::check(element, &format!("the stream element of `{}`", function.name))?;
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
        ty::pascal_case(function.names.ts.as_deref().unwrap_or(&function.name))
    );
    // Storage spells the user's own element type; only the value handed to
    // JavaScript picks up the top-level `Buffer` shape.
    let element_decl = ty::decl(element, ctx, Level::Nested)?;
    let element_top = ty::decl(element, ctx, Level::Top)?;
    let element_ret = ty::ret(element, ctx, &quote!(value));

    let wrapper_fn = function::render_fn(function, ctx)?;
    let class_docs = doc_attrs(&[format!(
        "Pull handle over the stream returned by `{}`.",
        function.name
    )]);
    let class_item = stream_class(&class, &js_class, &class_docs, &element_decl, &element_top, &element_ret);
    Ok(quote! {
        #wrapper_fn
        #class_item
    })
}

/// The generated handle class: checked-out pull state, a pull gate, and a
/// level-triggered closed flag.
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
            stream: ::std::sync::Mutex<
                ::std::option::Option<::unibind_runtime::UniStream<#element_decl>>,
            >,
            pull: ::tokio::sync::Mutex<()>,
            closed: ::tokio::sync::watch::Sender<bool>,
        }

        impl #class {
            fn __unibind_from(stream: ::unibind_runtime::UniStream<#element_decl>) -> Self {
                Self {
                    stream: ::std::sync::Mutex::new(::std::option::Option::Some(stream)),
                    pull: ::tokio::sync::Mutex::new(()),
                    closed: ::tokio::sync::watch::Sender::new(false),
                }
            }

            fn __unibind_slot(
                &self,
            ) -> ::std::sync::MutexGuard<
                '_,
                ::std::option::Option<::unibind_runtime::UniStream<#element_decl>>,
            > {
                self.stream
                    .lock()
                    .unwrap_or_else(::std::sync::PoisonError::into_inner)
            }
        }

        #[::napi_derive::napi]
        impl #class {
            /// The next element, or `null` once the stream ends or closes.
            // napi's impl expansion only exports methods that carry their own
            // `#[napi]` attribute, so the marker below is load-bearing.
            #[::napi_derive::napi]
            pub async fn next(&self) -> ::std::option::Option<#element_top> {
                let _pull = self.pull.lock().await;
                let mut stream = self.__unibind_slot().take()?;
                let mut closed = self.closed.subscribe();
                let item = ::tokio::select! {
                    biased;
                    _ = closed.wait_for(|closed| *closed) => ::std::option::Option::None,
                    item = stream.next() => item,
                };
                if item.is_some() && !*self.closed.borrow() {
                    self.__unibind_slot().replace(stream);
                }
                let value = item?;
                ::std::option::Option::Some(#element_ret)
            }

            /// Drop the stream early; a pull in flight resolves `null`, and
            /// the producer sees its stream dropped.
            #[::napi_derive::napi]
            pub fn close(&self) {
                let _ = self.closed.send(true);
                self.__unibind_slot().take();
            }
        }
    }
}
