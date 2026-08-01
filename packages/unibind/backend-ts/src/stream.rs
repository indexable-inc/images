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
//! Free functions and object methods both stream. The class is named per
//! export and scoped by its owner, because a method's class cannot live
//! inside the object's `#[napi] impl` block: every class renders at the
//! glue module's top level, from the one collection below.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::ir;
use unibind_core::render::{RenderError, pascal_case};

use crate::function::doc_attrs;
use crate::ty::{self, Level, TyCtx};

/// One stream-returning export: the callable that produced it plus the
/// element type its class yields.
pub struct StreamExport<'a> {
    /// `None` for free functions, the owning object's Rust name for
    /// methods; scopes the class name.
    pub owner: Option<&'a str>,
    /// The stream-returning callable.
    pub function: &'a ir::Function,
    /// The yielded element type.
    pub element: &'a ir::Type,
}

/// Every stream-returning export in the interface, in render order (free
/// functions first, then each object's methods).
pub fn collect(interface: &ir::Interface) -> Vec<StreamExport<'_>> {
    let free = interface
        .functions
        .iter()
        .filter_map(|function| stream_export(None, function));
    let methods = interface.objects.iter().flat_map(|object| {
        object
            .methods
            .iter()
            .filter_map(|method| stream_export(Some(object.name.as_str()), method))
    });
    free.chain(methods).collect()
}

fn stream_export<'a>(
    owner: Option<&'a str>,
    function: &'a ir::Function,
) -> Option<StreamExport<'a>> {
    let Some(ir::Type::Stream(element)) = &function.ret else {
        return None;
    };
    Some(StreamExport {
        owner,
        function,
        element,
    })
}

impl StreamExport<'_> {
    /// The handle class for this export: a napi shell over
    /// `unibind_runtime::PullStream`, which owns the pull and close
    /// mechanics.
    ///
    /// The element's napi representability is checked with the wrapper's
    /// return type (streams recurse in [`ty::check`]); only the
    /// object-element rule below is specific to the class, since an object
    /// name is a legal type everywhere else.
    pub fn render(&self, ctx: &TyCtx<'_>) -> Result<TokenStream, RenderError> {
        let produced = self.produced();
        if let ir::Type::Named(name) = self.element
            && ctx.object(name).is_some()
        {
            return Err(RenderError::new(format!(
                "`{produced}` streams objects; streams carry data (records \
                 and primitives) for now (issue #1993)"
            )));
        }
        let class = ty::stream_class_ident(self.owner, &self.function.name);
        let js_class = self.js_class(ctx);
        // Storage spells the user's own element type; only the value handed
        // to JavaScript picks up the top-level `Buffer` shape.
        let element_decl = ty::decl(self.element, ctx, Level::Nested)?;
        let element_top = ty::decl(self.element, ctx, Level::Top)?;
        let element_ret = ty::ret(self.element, ctx, &quote!(value));
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

    /// How diagnostics and docs name the export: `tail` for a free
    /// function, `Session.tail` for a method.
    fn produced(&self) -> String {
        self.owner.map_or_else(
            || self.function.name.clone(),
            |object| format!("{object}.{}", self.function.name),
        )
    }

    /// The JavaScript-visible class name: `TailStream` for a free `tail`,
    /// `SessionTailStream` for `Session::tail`. Renames apply, since a
    /// renamed export is the name JavaScript knows the stream by.
    fn js_class(&self, ctx: &TyCtx<'_>) -> String {
        let export = pascal_case(
            self.function
                .names
                .ts
                .as_deref()
                .unwrap_or(&self.function.name),
        );
        let owner = self.owner.and_then(|name| ctx.object(name)).map(|object| {
            object
                .names
                .ts
                .as_deref()
                .unwrap_or(object.name.as_str())
                .to_owned()
        });
        owner.map_or_else(
            || format!("{export}Stream"),
            |object| format!("{object}{export}Stream"),
        )
    }
}
