//! Per-export handle classes for `UniStream`-returning exports.
//!
//! Pull semantics: JavaScript calls `next()` and gets a promise of the next
//! element or `null` at the end, so nothing is produced faster than the
//! consumer asks (backpressure falls out of the pull). The pull and close
//! mechanics live in `unibind_runtime::PullStream`; the classes here are the
//! thin `wasm-bindgen` shells that name the element type. The generated
//! JavaScript wraps a handle into a real `AsyncIterable` -- the `null` is what
//! it turns into `{ done: true }` -- so the surface stays minimal on purpose:
//! `next` and `close`.
//!
//! The `PullStream` sits behind an `Arc` the ts backend does not need: `next()`
//! hands back a `Promise`, whose future is `'static` and so cannot borrow
//! `&self`.
//!
//! Free functions and object methods both stream. Which exports those are comes
//! from `unibind_core::render::stream_exports`, shared with the other backends;
//! every class renders at the glue module's top level, because a method's class
//! cannot live inside the object's own exported impl.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{Ownership, RenderError, StreamExport, pascal_case, rust_type_in};

use crate::function::doc_attrs;
use crate::ty::{self, TyCtx};
use crate::names;

/// The handle class for one stream-returning export.
///
/// # Errors
///
/// Fails for a stream of objects. The element's representability is checked with
/// the wrapper's return type ([`ty::check`] recurses through streams); only the
/// object-element rule is specific to the class, since an object name is a legal
/// type everywhere else. Lowering refuses this too, so the guard is the second
/// of two.
pub fn render(export: &StreamExport<'_>, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
    let produced = export.qualified_name();
    if let ir::Type::Named(name) = export.item
        && ctx.object(name).is_some()
    {
        return Err(RenderError::new(format!(
            "`{produced}` streams objects; streams carry data (records and \
             primitives) for now (issue #1993)"
        )));
    }
    let class = ty::stream_class_ident(export.owner, &export.function.name);
    let js_class = js_class(export, ctx);
    // Storage spells the user's own element type, which is what the producer
    // yields; only the value handed to JavaScript picks up the boundary shapes
    // (a `Uint8Array`, a checked `f64`, a record's twin).
    let user = ctx.user;
    let element_decl = rust_type_in(export.item, &quote!(#user), Ownership::Owned);
    let settle = ty::returned(export.item, ctx)?;
    let item = ty::resolved(Some(export.item), ctx, Some(&settle));
    let class_docs = doc_attrs(&[format!(
        " Pull handle over the stream returned by `{produced}`."
    )]);
    Ok(quote! {
        #class_docs
        #[::wasm_bindgen::prelude::wasm_bindgen(js_name = #js_class)]
        pub struct #class {
            stream: ::std::sync::Arc<::unibind_runtime::PullStream<#element_decl>>,
        }

        impl #class {
            fn __unibind_from(stream: ::unibind_runtime::UniStream<#element_decl>) -> Self {
                Self {
                    stream: ::std::sync::Arc::new(
                        ::unibind_runtime::PullStream::new(stream),
                    ),
                }
            }
        }

        #[::wasm_bindgen::prelude::wasm_bindgen(js_class = #js_class)]
        impl #class {
            /// The next element, or `null` once the stream ends or closes.
            #[wasm_bindgen(js_name = "next")]
            pub fn next(&self) -> ::js_sys::Promise {
                let __unibind_stream = ::std::sync::Arc::clone(&self.stream);
                ::wasm_bindgen_futures::future_to_promise(async move {
                    match __unibind_stream.next().await {
                        ::std::option::Option::Some(value) => #item,
                        ::std::option::Option::None => {
                            ::std::result::Result::Ok(::wasm_bindgen::JsValue::NULL)
                        }
                    }
                })
            }

            /// Drop the stream early; a pull in flight resolves `null`, and the
            /// producer sees its stream dropped.
            #[wasm_bindgen(js_name = "close")]
            pub fn close(&self) {
                self.stream.close();
            }
        }
    })
}

/// The JavaScript-visible class name: `TailStream` for a free `tail`,
/// `SessionTailStream` for `Session::tail`. Renames apply, since a renamed
/// export is the name JavaScript knows the stream by.
fn js_class(export: &StreamExport<'_>, ctx: &TyCtx<'_>) -> String {
    let name = pascal_case(&names::js_member(
        &export.function.names,
        &export.function.name,
    ));
    let owner = export
        .owner
        .and_then(|object| ctx.object(object))
        .map(|object| names::js_type(&object.names, &object.name));
    owner.map_or_else(
        || format!("{name}Stream"),
        |object| format!("{object}{name}Stream"),
    )
}
