//! Expansion pipeline: parse once, lower to IR, dispatch to backends.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::LowerError;

pub fn export(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut module = match syn::parse2::<syn::ItemMod>(item.clone()) {
        Ok(module) => module,
        Err(error) => {
            let error = error.to_compile_error();
            return quote! { #item #error };
        }
    };
    let selected = match unibind_core::export_backends(args.clone()) {
        Ok(selected) => selected,
        Err(error) => return with_error(&mut module, &error),
    };
    let interface = match unibind_core::lower_module(args, &module) {
        Ok(interface) => interface,
        Err(error) => return with_error(&mut module, &error),
    };
    unibind_core::strip_unibind_attrs(&mut module);
    let embed = match unibind_core::embed::ir_static(&interface) {
        Ok(embed) => embed,
        Err(error) => return with_error(&mut module, &error),
    };
    let glue = match backends(&interface, &mut module, selected.as_deref()) {
        Ok(glue) => glue,
        Err(error) => return with_error(&mut module, &error),
    };
    quote! {
        #module
        #embed
        #glue
    }
}

/// Emit the module (markers stripped, so nothing cascades) plus the
/// positioned diagnostic.
fn with_error(module: &mut syn::ItemMod, error: &LowerError) -> TokenStream {
    unibind_core::strip_unibind_attrs(module);
    let error = syn::Error::new(error.span, &error.message).to_compile_error();
    quote! { #module #error }
}

/// Render the selected backends; each contributes glue items and splices
/// its own record attributes.
///
/// `selected` is the `backends(...)` list from the attribute. Without it,
/// every feature-enabled backend renders -- fine for a crate built alone,
/// but a whole-workspace build unifies cargo features across every unibind
/// consumer, so a workspace mixing backend features needs each export to
/// name its own (or it would render glue whose runtime deps the crate
/// never declared). With no backend at all the macro still validates the
/// surface and embeds the IR; there is just no binding code to add.
fn backends(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    selected: Option<&[unibind_core::Backend]>,
) -> Result<TokenStream, LowerError> {
    let selects = |backend| selected.is_none_or(|backends| backends.contains(&backend));
    let mut glue = TokenStream::new();
    if selects(unibind_core::Backend::Py) {
        glue.extend(backend_py(interface, module, selected.is_some())?);
    }
    if selects(unibind_core::Backend::Ts) {
        glue.extend(backend_ts(interface, module, selected.is_some())?);
    }
    Ok(glue)
}

#[cfg(feature = "py")]
fn backend_py(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    _explicit: bool,
) -> Result<TokenStream, LowerError> {
    let rendered = unibind_backend_py::render(interface).map_err(|error| LowerError {
        span: proc_macro2::Span::call_site(),
        message: error.message,
    })?;
    splice_record_attrs(
        interface,
        module,
        rendered.records.iter().map(|record| RecordAttrs {
            outer: &record.outer,
            fields: &record.fields,
        }),
    );
    Ok(rendered.glue)
}

#[cfg(not(feature = "py"))]
fn backend_py(
    _interface: &unibind_core::ir::Interface,
    _module: &mut syn::ItemMod,
    explicit: bool,
) -> Result<TokenStream, LowerError> {
    if explicit {
        return Err(LowerError {
            span: proc_macro2::Span::call_site(),
            message: "backends(py) needs the `py` cargo feature of unibind".to_owned(),
        });
    }
    Ok(TokenStream::new())
}

#[cfg(feature = "ts")]
fn backend_ts(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    _explicit: bool,
) -> Result<TokenStream, LowerError> {
    let rendered = unibind_backend_ts::render(interface).map_err(|error| LowerError {
        span: proc_macro2::Span::call_site(),
        message: error.message,
    })?;
    splice_record_attrs(
        interface,
        module,
        rendered.records.iter().map(|record| RecordAttrs {
            outer: &record.outer,
            fields: &record.fields,
        }),
    );
    Ok(rendered.glue)
}

#[cfg(not(feature = "ts"))]
fn backend_ts(
    _interface: &unibind_core::ir::Interface,
    _module: &mut syn::ItemMod,
    explicit: bool,
) -> Result<TokenStream, LowerError> {
    if explicit {
        return Err(LowerError {
            span: proc_macro2::Span::call_site(),
            message: "backends(ts) needs the `ts` cargo feature of unibind".to_owned(),
        });
    }
    Ok(TokenStream::new())
}

/// One record's backend-rendered attributes, index-aligned with the
/// record's fields.
#[cfg(any(feature = "py", feature = "ts"))]
struct RecordAttrs<'a> {
    outer: &'a [syn::Attribute],
    fields: &'a [Vec<syn::Attribute>],
}

/// Attach a backend's `#[pyclass]`- or `#[napi(object)]`-shaped attributes
/// to the record structs the IR was lowered from. Records and rendered
/// attribute sets are index-aligned by construction.
#[cfg(any(feature = "py", feature = "ts"))]
fn splice_record_attrs<'a>(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    records: impl Iterator<Item = RecordAttrs<'a>>,
) {
    let Some((_, items)) = &mut module.content else {
        return;
    };
    for (record, attrs) in interface.records.iter().zip(records) {
        for item in &mut *items {
            let syn::Item::Struct(item) = item else {
                continue;
            };
            if item.ident != record.name {
                continue;
            }
            let mut outer = attrs.outer.to_vec();
            outer.append(&mut item.attrs);
            item.attrs = outer;
            for (field, field_attrs) in item.fields.iter_mut().zip(attrs.fields) {
                field.attrs.extend(field_attrs.iter().cloned());
            }
        }
    }
}

pub fn marker_outside_export(item: TokenStream, message: &str) -> TokenStream {
    let error = syn::Error::new(proc_macro2::Span::call_site(), message).to_compile_error();
    quote! { #item #error }
}
