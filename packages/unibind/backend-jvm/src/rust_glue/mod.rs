//! Assemble the hidden glue module of plain `extern "C"` exports.

mod asserts;
mod asyncfn;
mod decode;
mod encode;
mod envelope;
mod function;
mod object;
mod rusty;
mod stream;
mod types;

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::model::Model;
use crate::{names, RenderedJvm, RenderError};

/// One `extern "C"` export site: a free function, an object constructor,
/// or an object method. Constructors and methods share the free-function
/// machinery; only the receiver and the envelope naming differ.
pub(crate) struct Export<'a> {
    /// The owning object's name; `None` for free functions.
    pub owner: Option<&'a str>,
    pub function: &'a ir::Function,
    /// The boundary success type: the function's `ret`, or the object's
    /// own type for constructors (whose IR `ret` is `None`).
    pub ret: Option<ir::Type>,
    /// Whether the export takes a leading `this` handle (methods only).
    pub has_receiver: bool,
}

impl Export<'_> {
    /// The base `extern "C"` symbol; companions suffix onto it.
    pub(crate) fn symbol(&self, interface: &ir::Interface) -> String {
        match self.owner {
            Some(object) => {
                names::object_export_symbol(&interface.name, object, &self.function.name)
            }
            None => names::export_symbol(&interface.name, &self.function.name),
        }
    }

    /// A human-readable call site for docs and errors.
    pub(crate) fn site(&self) -> String {
        self.owner.map_or_else(
            || self.function.name.clone(),
            |object| format!("{object}.{}", self.function.name),
        )
    }
}

/// Every export site in the interface, in render order: free functions,
/// then each object's constructor and methods.
pub(crate) fn exports(interface: &ir::Interface) -> Vec<Export<'_>> {
    let mut out = Vec::new();
    for function in &interface.functions {
        out.push(Export {
            owner: None,
            function,
            ret: function.ret.clone(),
            has_receiver: false,
        });
    }
    for object in &interface.objects {
        if let Some(ctor) = &object.constructor {
            out.push(Export {
                owner: Some(&object.name),
                function: ctor,
                ret: Some(ir::Type::Named(object.name.clone())),
                has_receiver: false,
            });
        }
        for method in &object.methods {
            out.push(Export {
                owner: Some(&object.name),
                function: method,
                ret: method.ret.clone(),
                has_receiver: true,
            });
        }
    }
    out
}

/// Render the `extern "C"` glue for one interface.
///
/// # Errors
///
/// Fails for surface the JVM backend does not implement (data enums) and
/// for types that cannot cross the boundary (streams outside return
/// position, objects outside handle position, unresolved or recursive
/// records).
pub fn render(interface: &ir::Interface) -> Result<RenderedJvm, RenderError> {
    let model = Model::new(interface)?;
    let user = names::rust_ident(&interface.name)?;
    let glue_ident = format_ident!("__unibind_jvm_{}", interface.name.trim_start_matches('_'));
    let sites = exports(interface);

    let runtime = types::runtime();
    let records = types::record_mirrors(interface)?;
    let envelopes = types::envelopes(&model, &sites);
    let asserts = asserts::layout_asserts(interface, &model, &sites);
    let decode_helpers = decode::helpers();
    let encode_helpers = encode::helpers();
    let panic_helpers = function::helpers();
    let mut items = Vec::new();
    for export in &sites {
        items.push(render_export(export, interface, &model, &user)?);
    }
    for object in &interface.objects {
        items.push(object::free_export(object, interface, &user)?);
    }
    let abi = function::abi_version(&interface.name);
    let module_doc = format!(
        "unibind JVM glue for `{}`: `extern \"C\"` exports consumed by the generated Java \
         Panama binding.",
        interface.name
    );

    let glue = quote! {
        #[doc = #module_doc]
        #[doc(hidden)]
        #[allow(
            clippy::all,
            clippy::pedantic,
            clippy::nursery,
            dead_code,
            missing_docs,
            unsafe_code,
            unused_qualifications
        )]
        mod #glue_ident {
            #runtime
            #records
            #envelopes
            #asserts
            #decode_helpers
            #encode_helpers
            #panic_helpers
            #(#items)*
            #abi
        }
    };
    Ok(RenderedJvm { glue })
}

/// One export's items: the base export by asyncness, plus the stream
/// companions when it returns a stream.
fn render_export(
    export: &Export<'_>,
    interface: &ir::Interface,
    model: &Model<'_>,
    user: &proc_macro2::Ident,
) -> Result<TokenStream, RenderError> {
    let base = match export.function.asyncness {
        ir::Asyncness::Sync => function::render_sync(export, interface, model, user)?,
        ir::Asyncness::Async => asyncfn::render_async(export, interface, model, user)?,
    };
    let companions = match &export.ret {
        Some(ir::Type::Stream(item)) => {
            Some(stream::companions(export, interface, model, user, item)?)
        }
        _ => None,
    };
    Ok(quote!(#base #companions))
}
