//! Assemble the glue module: the bridge module plus the handles and
//! wrappers it dispatches to.

use proc_macro2::{Ident, Span, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::{boxes, error, function, names, record, repr, swift, RenderError, RenderedInterface};

/// Render `swift-bridge` glue for one interface.
///
/// # Errors
///
/// Fails for surface the phase 7 backend does not implement (async
/// functions, data enums, objects) and for names that cannot become
/// identifiers.
pub fn render(interface: &ir::Interface) -> Result<RenderedInterface, RenderError> {
    if let Some(object) = interface.objects.first() {
        return Err(RenderError::new(format!(
            "`{}` is a #[unibind::object]; objects land with the swift backend's \
             async follow-up (issue #2082)",
            object.name
        )));
    }
    if let Some(data_enum) = interface.enums.first() {
        return Err(RenderError::new(format!(
            "`{}` is a data enum, which the swift backend does not render",
            data_enum.name
        )));
    }
    reject_streams(interface)?;

    let user = names::name_ident(&interface.name)?;
    let glue_ident = format_ident!("__unibind_swift_{}", interface.name.trim_start_matches('_'));
    let ffi_mod = Ident::new("__unibind_ffi", Span::call_site());

    let errors = interface
        .errors
        .iter()
        .map(|err| error::render_error(err, &ffi_mod, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let records = interface
        .records
        .iter()
        .map(|rec| record::render_record(rec, &user))
        .collect::<Result<Vec<_>, _>>()?;
    let box_shapes = repr::collect_boxes(interface);
    let rendered_boxes: Vec<_> = box_shapes
        .values()
        .map(|shape| boxes::render_box(shape, &user))
        .collect();
    let functions = interface
        .functions
        .iter()
        .map(|func| function::render_fn(func, interface, &ffi_mod, &user))
        .collect::<Result<Vec<_>, _>>()?;

    let bridge_enums = errors.iter().map(|err| &err.bridge_enum);
    let record_decls = records.iter().map(|rec| &rec.bridge_decls);
    let box_decls = rendered_boxes.iter().map(|rendered| &rendered.bridge_decls);
    let fn_decls = functions.iter().map(|func| &func.bridge_decl);
    // The attribute stays `swift_bridge::bridge` (no leading `::`):
    // swift-bridge-build recognizes bridge modules by comparing the attribute
    // path's token text, and a leading `::` would make it skip the module.
    let bridge = quote! {
        #[swift_bridge::bridge]
        mod #ffi_mod {
            #(#bridge_enums)*
            extern "Rust" {
                #(#record_decls)*
                #(#box_decls)*
                #(#fn_decls)*
            }
        }
    };

    let record_items = records.iter().map(|rec| &rec.items);
    let box_items = rendered_boxes.iter().map(|rendered| &rendered.items);
    let error_items = errors.iter().map(|err| &err.items);
    let fn_items = functions.iter().map(|func| &func.item);
    let glue = quote! {
        #[doc(hidden)]
        #[allow(clippy::all, clippy::pedantic, clippy::nursery, dead_code, unused_qualifications)]
        mod #glue_ident {
            #bridge

            #(#record_items)*
            #(#box_items)*
            #(#error_items)*
            #(#fn_items)*
        }
    };
    let overlay = swift::render(interface);
    Ok(RenderedInterface {
        glue,
        bridge,
        overlay,
    })
}

/// Refuse `UniStream` anywhere in the surface: streams render as Swift
/// `AsyncSequence` in the async follow-up of issue #2082.
fn reject_streams(interface: &ir::Interface) -> Result<(), RenderError> {
    let field_types = interface
        .records
        .iter()
        .flat_map(|record| record.fields.iter())
        .map(|field| (&field.name, &field.ty));
    let arg_types = interface
        .functions
        .iter()
        .flat_map(|function| function.args.iter().map(move |arg| (&function.name, &arg.ty)));
    let ret_types = interface
        .functions
        .iter()
        .filter_map(|function| function.ret.as_ref().map(|ret| (&function.name, ret)));
    for (name, ty) in field_types.chain(arg_types).chain(ret_types) {
        if mentions_stream(ty) {
            return Err(RenderError::new(format!(
                "`{name}` uses a UniStream; streams land with the swift backend's \
                 async follow-up (issue #2082)"
            )));
        }
    }
    Ok(())
}

fn mentions_stream(ty: &ir::Type) -> bool {
    match ty {
        ir::Type::Stream(_) => true,
        ir::Type::Option(inner) | ir::Type::Vec(inner) => mentions_stream(inner),
        ir::Type::Map { key, value } => mentions_stream(key) || mentions_stream(value),
        ir::Type::Bool
        | ir::Type::Int(_)
        | ir::Type::Float(_)
        | ir::Type::String { .. }
        | ir::Type::Path { .. }
        | ir::Type::Bytes { .. }
        | ir::Type::Named(_) => false,
    }
}
