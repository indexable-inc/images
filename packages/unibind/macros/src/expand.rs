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
    let (args, selection) = match split_backends(args) {
        Ok(split) => split,
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
    let glue = match backends(&interface, &mut module, selection) {
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

/// Which backends `backends(...)` selected on the export attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BackendSelection {
    py: bool,
    jvm: bool,
}

/// Pull a `backends(py, jvm)` entry out of the export arguments, leaving
/// the rest (e.g. `py(name = "...")`) for lowering. `None` means the module
/// renders every feature-enabled backend. Arguments that do not parse as an
/// option list pass through untouched so lowering owns their diagnostics.
fn split_backends(args: TokenStream) -> Result<(TokenStream, Option<BackendSelection>), LowerError> {
    use syn::spanned::Spanned as _;
    if args.is_empty() {
        return Ok((args, None));
    }
    let parser = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;
    let Ok(entries) = syn::parse::Parser::parse2(parser, args.clone()) else {
        return Ok((args, None));
    };
    let mut rest: Vec<syn::Meta> = Vec::new();
    let mut selection = None;
    for entry in entries {
        if !entry.path().is_ident("backends") {
            rest.push(entry);
            continue;
        }
        let span = entry.span();
        if selection.is_some() {
            return Err(LowerError {
                span,
                message: "duplicate unibind `backends(...)`".to_owned(),
            });
        }
        let syn::Meta::List(list) = entry else {
            return Err(LowerError {
                span,
                message: "`backends` takes a list: backends(py), backends(jvm), or backends(py, jvm)"
                    .to_owned(),
            });
        };
        let names = syn::parse::Parser::parse2(
            syn::punctuated::Punctuated::<syn::Ident, syn::Token![,]>::parse_terminated,
            list.tokens.clone(),
        )
        .map_err(|error| LowerError {
            span,
            message: format!("bad `backends` options: {error}"),
        })?;
        let mut selected = BackendSelection {
            py: false,
            jvm: false,
        };
        for name in &names {
            match name.to_string().as_str() {
                "py" => selected.py = true,
                "jvm" => selected.jvm = true,
                other => {
                    return Err(LowerError {
                        span: name.span(),
                        message: format!("unknown backend `{other}`; expected `py` or `jvm`"),
                    });
                }
            }
        }
        if selected == (BackendSelection { py: false, jvm: false }) {
            return Err(LowerError {
                span,
                message: "`backends(...)` needs at least one of `py`, `jvm`".to_owned(),
            });
        }
        selection = Some(selected);
    }
    let rest = quote!(#(#rest),*);
    Ok((rest, selection))
}

/// Concatenate the glue of the selected backends: the `backends(...)`
/// argument when given, every feature-enabled backend otherwise. Cargo
/// unifies the facade's features across a workspace, so a module whose
/// surface only one backend renders (e.g. async python bindings while the
/// JVM backend is still sync-only) declares that backend explicitly instead
/// of inheriting a neighbor crate's features. With no backend feature the
/// macro still validates the surface and embeds the IR; there is just no
/// binding code to add.
fn backends(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    selection: Option<BackendSelection>,
) -> Result<TokenStream, LowerError> {
    let explicit = selection.is_some();
    let mut glue = TokenStream::new();
    if selection.is_none_or(|selected| selected.py) {
        glue.extend(py_glue(interface, module, explicit).map_err(|error| implicit_hint(error, explicit, "py"))?);
    }
    if selection.is_none_or(|selected| selected.jvm) {
        glue.extend(jvm_glue(interface, explicit).map_err(|error| implicit_hint(error, explicit, "jvm"))?);
    }
    Ok(glue)
}

/// A render failure from a backend nobody asked for by name deserves the
/// way out: point at the `backends(...)` selection.
fn implicit_hint(error: LowerError, explicit: bool, backend: &str) -> LowerError {
    if explicit {
        return error;
    }
    LowerError {
        span: error.span,
        message: format!(
            "{message}\n(the `{backend}` backend came in through cargo feature \
             unification; if this module should not render it, select the \
             intended backends explicitly: #[unibind::export(backends(...))])",
            message = error.message
        ),
    }
}

#[cfg(feature = "py")]
fn py_glue(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    _explicit: bool,
) -> Result<TokenStream, LowerError> {
    let rendered = unibind_backend_py::render(interface).map_err(|error| LowerError {
        span: proc_macro2::Span::call_site(),
        message: error.message,
    })?;
    splice_record_attrs(interface, module, &rendered);
    Ok(rendered.glue)
}

#[cfg(not(feature = "py"))]
fn py_glue(
    _interface: &unibind_core::ir::Interface,
    _module: &mut syn::ItemMod,
    explicit: bool,
) -> Result<TokenStream, LowerError> {
    if explicit {
        return Err(LowerError {
            span: proc_macro2::Span::call_site(),
            message: "`backends(py)` needs the unibind `py` cargo feature: \
                      unibind = { features = [\"py\"] }"
                .to_owned(),
        });
    }
    Ok(TokenStream::new())
}

/// The JVM glue is self-contained `extern "C"` exports; unlike `py` it
/// attaches nothing to the record structs.
#[cfg(feature = "jvm")]
fn jvm_glue(
    interface: &unibind_core::ir::Interface,
    _explicit: bool,
) -> Result<TokenStream, LowerError> {
    let rendered = unibind_backend_jvm::render(interface).map_err(|error| LowerError {
        span: proc_macro2::Span::call_site(),
        message: error.message,
    })?;
    Ok(rendered.glue)
}

#[cfg(not(feature = "jvm"))]
fn jvm_glue(
    _interface: &unibind_core::ir::Interface,
    explicit: bool,
) -> Result<TokenStream, LowerError> {
    if explicit {
        return Err(LowerError {
            span: proc_macro2::Span::call_site(),
            message: "`backends(jvm)` needs the unibind `jvm` cargo feature: \
                      unibind = { features = [\"jvm\"] }"
                .to_owned(),
        });
    }
    Ok(TokenStream::new())
}

/// Attach the backend's `#[pyclass]`-shaped attributes to the record
/// structs the IR was lowered from. Records and rendered attribute sets are
/// index-aligned by construction.
#[cfg(feature = "py")]
fn splice_record_attrs(
    interface: &unibind_core::ir::Interface,
    module: &mut syn::ItemMod,
    rendered: &unibind_backend_py::RenderedInterface,
) {
    let Some((_, items)) = &mut module.content else {
        return;
    };
    for (record, attrs) in interface.records.iter().zip(&rendered.records) {
        for item in &mut *items {
            let syn::Item::Struct(item) = item else {
                continue;
            };
            if item.ident != record.name {
                continue;
            }
            let mut outer = attrs.outer.clone();
            outer.append(&mut item.attrs);
            item.attrs = outer;
            for (field, field_attrs) in item.fields.iter_mut().zip(&attrs.fields) {
                field.attrs.extend(field_attrs.iter().cloned());
            }
        }
    }
}

pub fn marker_outside_export(item: TokenStream, message: &str) -> TokenStream {
    let error = syn::Error::new(proc_macro2::Span::call_site(), message).to_compile_error();
    quote! { #item #error }
}
