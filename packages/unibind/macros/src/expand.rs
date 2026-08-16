//! Expansion pipeline: parse once, lower to IR, dispatch to backends.

use proc_macro2::TokenStream;
use quote::quote;
use unibind_core::LowerError;

pub fn export(args: TokenStream, item: &TokenStream) -> TokenStream {
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
    // Parts land in the module before anything reads it, so lowering, the
    // marker strip and the record-attribute splice all see one surface
    // whatever it was split over.
    let listed = match unibind_core::export_parts(args.clone()) {
        Ok(listed) => listed,
        Err(error) => return with_error(&mut module, &error),
    };
    let part_inputs = match crate::parts::splice(&mut module, &listed) {
        Ok(inputs) => inputs,
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
        #part_inputs
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
        let interface = &resolve_docs(interface, unibind_core::docs::Language::Py)?;
        glue.extend(backend_py(interface, module, selected.is_some())?);
    }
    if selects(unibind_core::Backend::Ts) {
        let interface = &resolve_docs(interface, unibind_core::docs::Language::Ts)?;
        glue.extend(backend_ts(interface, module, selected.is_some())?);
    }
    if selects(unibind_core::Backend::Ex) {
        let interface = &resolve_docs(interface, unibind_core::docs::Language::Ex)?;
        glue.extend(backend_ex(interface, module, selected.is_some())?);
    }
    if selects(unibind_core::Backend::Jvm) {
        let interface = &resolve_docs(interface, unibind_core::docs::Language::Jvm)?;
        glue.extend(backend_jvm(interface, module, selected.is_some())?);
    }
    if selects(unibind_core::Backend::Wasm) {
        // The wasm backend deliberately shares TypeScript doc spelling: one
        // TS-facing vocabulary across node and browser, so `ts(name = ...)`
        // renames and `{@link ...}` targets agree between the two artifacts.
        let interface = &resolve_docs(interface, unibind_core::docs::Language::Ts)?;
        glue.extend(backend_wasm(interface, module, selected.is_some())?);
    }
    Ok(glue)
}

/// The interface with its doc comments spelled for one language, so the
/// `#[doc]` attributes the glue carries name the same identifiers the
/// generated host files do. Lowering already refused an unresolvable link,
/// so a failure here is one the diagnostic still has to name rather than a
/// case a caller can hit.
fn resolve_docs(
    interface: &unibind_core::ir::Interface,
    language: unibind_core::docs::Language,
) -> Result<unibind_core::ir::Interface, LowerError> {
    unibind_core::docs::resolve(interface, language).map_err(|error| LowerError {
        span: proc_macro2::Span::call_site(),
        message: error.to_string(),
    })
}

macro_rules! enabled_backend {
    ($name:ident, $feature:literal, $render:path) => {
        #[cfg(feature = $feature)]
        fn $name(
            interface: &unibind_core::ir::Interface,
            module: &mut syn::ItemMod,
            _explicit: bool,
        ) -> Result<TokenStream, LowerError> {
            let rendered = ($render)(interface).map_err(|error| LowerError {
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
    };
}

macro_rules! disabled_backend {
    ($name:ident, $feature:literal) => {
        #[cfg(not(feature = $feature))]
        fn $name(
            _interface: &unibind_core::ir::Interface,
            _module: &mut syn::ItemMod,
            explicit: bool,
        ) -> Result<TokenStream, LowerError> {
            if explicit {
                return Err(LowerError {
                    span: proc_macro2::Span::call_site(),
                    message: concat!(
                        "backends(",
                        $feature,
                        ") needs the `",
                        $feature,
                        "` cargo feature of unibind"
                    )
                    .to_owned(),
                });
            }
            Ok(TokenStream::new())
        }
    };
}

enabled_backend!(backend_py, "py", unibind_backend_py::render);
disabled_backend!(backend_py, "py");
enabled_backend!(backend_ts, "ts", unibind_backend_ts::render);
disabled_backend!(backend_ts, "ts");
enabled_backend!(backend_jvm, "jvm", unibind_backend_jvm::render);
disabled_backend!(backend_jvm, "jvm");
enabled_backend!(backend_wasm, "wasm", unibind_backend_wasm::render);
disabled_backend!(backend_wasm, "wasm");

#[cfg(feature = "ex")]
fn backend_ex(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    _explicit: bool,
) -> Result<TokenStream, LowerError> {
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

disabled_backend!(backend_ex, "ex");

/// One record's backend-rendered attributes, index-aligned with the
/// record's fields.
#[cfg(any(feature = "py", feature = "ts", feature = "ex", feature = "jvm", feature = "wasm"))]
struct RecordAttrs<'a> {
    outer: &'a [syn::Attribute],
    fields: &'a [Vec<syn::Attribute>],
}

/// Attach a backend's `#[pyclass]`-, `#[napi(object)]`-, or
/// `#[derive(NifStruct)]`-shaped attributes to the record structs the IR
/// was lowered from, and replace their doc comments with the resolved ones.
///
/// Records and rendered attribute sets are index-aligned by construction.
///
/// The doc replacement is what makes a record's runtime documentation agree
/// with its stub. A record's `#[pyclass]` lands on the user's own struct, so
/// pyo3 reads that struct's `///` text for `help(Point)` and for each
/// getter -- the one doc site the generated wrappers do not own. `interface`
/// here is already resolved for this backend's language, so writing its doc
/// lines back over the struct's is all it takes for `help()` and the `.pyi`
/// to say the same thing.
#[cfg(any(feature = "py", feature = "ts", feature = "ex", feature = "jvm", feature = "wasm"))]
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
            replace_docs(&mut item.attrs, &record.docs);
            let mut outer = attrs.outer.to_vec();
            outer.append(&mut item.attrs);
            item.attrs = outer;
            for ((field, field_attrs), lowered) in item
                .fields
                .iter_mut()
                .zip(attrs.fields)
                .zip(&record.fields)
            {
                replace_docs(&mut field.attrs, &lowered.docs);
                field.attrs.extend(field_attrs.iter().cloned());
            }
        }
    }
}

/// Swap an item's doc comments for `lines`, keeping their position among
/// the other attributes.
///
/// The leading space rustdoc conventionally writes is put back, so the only
/// difference from the source text is the resolved links themselves.
#[cfg(any(feature = "py", feature = "ts", feature = "ex", feature = "jvm", feature = "wasm"))]
fn replace_docs(attributes: &mut Vec<syn::Attribute>, lines: &[String]) {
    let first = attributes
        .iter()
        .position(|attribute| attribute.path().is_ident("doc"));
    attributes.retain(|attribute| !attribute.path().is_ident("doc"));
    let Some(at) = first else {
        return;
    };
    let rendered = lines.iter().map(|line| -> syn::Attribute {
        let text = format!(" {line}");
        syn::parse_quote!(#[doc = #text])
    });
    let mut tail = attributes.split_off(at);
    attributes.extend(rendered);
    attributes.append(&mut tail);
}

pub fn marker_outside_export(item: &TokenStream, message: &str) -> TokenStream {
    let error = syn::Error::new(proc_macro2::Span::call_site(), message).to_compile_error();
    quote! { #item #error }
}
