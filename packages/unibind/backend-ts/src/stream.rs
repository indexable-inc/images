//! Per-export handle classes for `UniStream`-returning exports.
//!
//! Pull semantics: JavaScript calls `next()` and gets a promise of the next
//! element or `null` at the end, so nothing is produced faster than the
//! consumer asks (backpressure falls out of the pull). The pull and close
//! mechanics live in `unibind_runtime::PullStream`; the classes here are
//! the thin napi shells that name the element type. The generated
//! `index.js` wraps a handle into a real `AsyncIterable`; the surface stays
//! minimal on purpose: `next` and `close`.
//!
//! Free functions and object methods both stream. Which exports those are
//! comes from `unibind_core::render::stream_exports`, shared with the
//! Python backend; every class renders at the glue module's top level,
//! because a method's class cannot live inside the object's
//! `#[napi] impl` block.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{Ownership, RenderError, StreamExport, pascal_case, rust_type_in};

use crate::function::doc_attrs;
use crate::ty::{self, Level, TyCtx};

/// The handle class for one stream-returning export: a napi shell over
/// `unibind_runtime::PullStream`, which owns the pull and close mechanics.
///
/// # Errors
///
/// Fails for a stream of objects. The element's napi representability is
/// checked with the wrapper's return type ([`ty::check`] recurses through
/// streams); only the object-element rule is specific to the class, since
/// an object name is a legal type everywhere else.
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
    // Storage spells the user's own element type, which is what the
    // producer yields; only the value handed to JavaScript picks up the
    // boundary shapes (`Buffer`, checked `f64` integers, a record's mirror).
    let user = ctx.user;
    let element_decl = rust_type_in(export.item, &quote!(#user), Ownership::Owned);
    let element_top = ty::decl(export.item, ctx, Level::Top)?;
    let element_ret = ty::ret(export.item, ctx, &quote!(value));
    let class_docs = doc_attrs(&[format!(
        "Pull handle over the stream returned by `{produced}`."
    )]);
    Ok(quote! {
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
    })
}

/// The JavaScript-visible class name: `TailStream` for a free `tail`,
/// `SessionTailStream` for `Session::tail`. Renames apply, since a renamed
/// export is the name JavaScript knows the stream by.
fn js_class(export: &StreamExport<'_>, ctx: &TyCtx<'_>) -> String {
    let name = pascal_case(
        export
            .function
            .names
            .ts
            .as_deref()
            .unwrap_or(&export.function.name),
    );
    let owner = export
        .owner
        .and_then(|object| ctx.object(object))
        .map(|object| object.names.ts.as_deref().unwrap_or(&object.name));
    owner.map_or_else(
        || format!("{name}Stream"),
        |object| format!("{object}{name}Stream"),
    )
}
