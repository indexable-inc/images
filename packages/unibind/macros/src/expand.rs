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
    // Cargo unifies this crate's features across a workspace build, so a
    // consumer sharing a workspace with consumers of other backends names
    // its own targets with `backends(...)`; absent, every enabled backend
    // renders.
    let selection = match unibind_core::module_backends(args.clone(), proc_macro2::Span::call_site())
    {
        Ok(selection) => selection,
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
    let glue = match backends(&interface, &mut module, selection.as_deref()) {
        Ok(glue) => glue,
        Err(error) => return with_error(&mut module, &error),
    };
    quote! {
        #module
        #embed
        #glue
    }
}

/// Whether the export renders `backend`: named in the selection, or no
/// selection at all. A selected backend whose feature is off is an error
/// (the crate asked for glue this build cannot produce), raised in
/// [`backends`].
#[cfg(any(feature = "py", feature = "ex"))]
fn selected(selection: Option<&[String]>, backend: &str) -> bool {
    selection.is_none_or(|names| names.iter().any(|name| name == backend))
}

/// The backend names this build compiled in.
const fn enabled_backends() -> &'static [&'static str] {
    &[
        #[cfg(feature = "ex")]
        "ex",
        #[cfg(feature = "py")]
        "py",
    ]
}

/// Emit the module (markers stripped, so nothing cascades) plus the
/// positioned diagnostic.
fn with_error(module: &mut syn::ItemMod, error: &LowerError) -> TokenStream {
    unibind_core::strip_unibind_attrs(module);
    let error = syn::Error::new(error.span, &error.message).to_compile_error();
    quote! { #module #error }
}

/// Render every backend the consuming crate enabled, splicing each one's
/// record attributes and concatenating the glue.
fn backends(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    selection: Option<&[String]>,
) -> Result<TokenStream, LowerError> {
    if let Some(missing) = selection
        .unwrap_or_default()
        .iter()
        .find(|name| !enabled_backends().contains(&name.as_str()))
    {
        return Err(LowerError {
            span: proc_macro2::Span::call_site(),
            message: format!(
                "backends({missing}) needs the `{missing}` feature on the `unibind` dependency"
            ),
        });
    }
    let mut glue: Vec<TokenStream> = Vec::new();
    #[cfg(feature = "py")]
    if selected(selection, "py") {
        let rendered = unibind_backend_py::render(interface).map_err(|error| LowerError {
            span: proc_macro2::Span::call_site(),
            message: error.message,
        })?;
        let attrs = rendered
            .records
            .iter()
            .map(|record| RecordAttrs {
                outer: record.outer.clone(),
                fields: record.fields.clone(),
            })
            .collect();
        splice_record_attrs(interface, module, attrs);
        glue.push(rendered.glue);
    }
    #[cfg(feature = "ex")]
    if selected(selection, "ex") {
        // The consuming crate's name, for the plain `nif_init` alias; set
        // by every cargo-compatible driver, as rustler's own macro assumes.
        let crate_name = std::env::var("CARGO_CRATE_NAME").map_err(|_| LowerError {
            span: proc_macro2::Span::call_site(),
            message: "the ex backend needs CARGO_CRATE_NAME during expansion".to_owned(),
        })?;
        let rendered =
            unibind_backend_ex::render(interface, Some(&crate_name)).map_err(|error| LowerError {
            span: proc_macro2::Span::call_site(),
            message: error.message,
        })?;
        let attrs = rendered
            .records
            .iter()
            .map(|record| RecordAttrs {
                outer: record.outer.clone(),
                fields: record.fields.clone(),
            })
            .collect();
        splice_record_attrs(interface, module, attrs);
        glue.push(rendered.glue);
    }
    // With no backend feature enabled the macro still validates the surface
    // and embeds the IR; there is just no binding code to add.
    #[cfg(not(any(feature = "py", feature = "ex")))]
    {
        let _ = (interface, module);
    }
    Ok(quote! { #(#glue)* })
}

/// One backend's contribution to a record struct.
#[cfg(any(feature = "py", feature = "ex"))]
struct RecordAttrs {
    /// Outer attributes for the struct itself.
    outer: Vec<syn::Attribute>,
    /// Attributes for each field, index-aligned with the record's fields.
    fields: Vec<Vec<syn::Attribute>>,
}

/// Attach a backend's record attributes to the structs the IR was lowered
/// from. Records and rendered attribute sets are index-aligned by
/// construction.
#[cfg(any(feature = "py", feature = "ex"))]
fn splice_record_attrs(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    attrs: Vec<RecordAttrs>,
) {
    let Some((_, items)) = &mut module.content else {
        return;
    };
    for (record, rendered) in interface.records.iter().zip(attrs) {
        for item in &mut *items {
            let syn::Item::Struct(item) = item else {
                continue;
            };
            if item.ident != record.name {
                continue;
            }
            let mut outer = rendered.outer.clone();
            outer.append(&mut item.attrs);
            item.attrs = outer;
            for (field, attrs) in item.fields.iter_mut().zip(&rendered.fields) {
                field.attrs.extend(attrs.iter().cloned());
            }
        }
    }
}

pub fn marker_outside_export(item: TokenStream, message: &str) -> TokenStream {
    let error = syn::Error::new(proc_macro2::Span::call_site(), message).to_compile_error();
    quote! { #item #error }
}
