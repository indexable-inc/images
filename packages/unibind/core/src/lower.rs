//! Lower a `#[unibind::export]` module's syn items into [`crate::ir`].

mod attrs;
mod data;
mod func;
mod ty;

use proc_macro2::Span;
use syn::spanned::Spanned as _;

use crate::ir;

/// A lowering failure, positioned so the macro can emit `compile_error!` at
/// the offending tokens.
#[derive(Debug)]
pub struct LowerError {
    /// Where the diagnostic points.
    pub span: Span,
    /// What went wrong and what to do instead.
    pub message: String,
}

impl LowerError {
    pub(crate) fn new(span: Span, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, LowerError>;

/// Type names declared in the exported module, used to validate references.
#[derive(Debug, Default)]
pub struct Declared {
    pub records: Vec<String>,
    pub errors: Vec<String>,
}

/// Lower an inline `#[unibind::export]` module into an [`ir::Interface`].
///
/// `module_args` is the token stream the attribute itself carried, for
/// example `py(name = "...")` from `#[unibind::export(py(name = "..."))]`.
///
/// # Errors
///
/// Returns a positioned error for anything outside the phase 0 surface:
/// async functions, generics, methods, data enums, `#[unibind::object]`,
/// unsupported boundary types, or malformed `#[unibind(...)]` metadata.
pub fn lower_module(
    module_args: proc_macro2::TokenStream,
    module: &syn::ItemMod,
) -> Result<ir::Interface> {
    let meta = attrs::UnibindMeta::parse(module_args, module.span())?;
    meta.reject_default("a module")?;
    meta.reject_py_base("a module")?;
    let Some((_, items)) = &module.content else {
        return Err(LowerError::new(
            module.span(),
            "#[unibind::export] needs an inline module (`mod name { ... }`); \
             out-of-line modules are not supported",
        ));
    };

    let declared = collect_declared(items)?;
    let mut interface = ir::Interface {
        version: ir::IR_VERSION,
        name: module.ident.to_string(),
        names: meta.names(),
        docs: attrs::doc_lines(&module.attrs),
        functions: Vec::new(),
        records: Vec::new(),
        enums: Vec::new(),
        errors: Vec::new(),
        objects: Vec::new(),
    };

    for item in items {
        match (item, attrs::marker(item)?) {
            (syn::Item::Fn(func), None) => {
                if matches!(func.vis, syn::Visibility::Public(_)) {
                    interface.functions.push(func::lower_fn(func, &declared)?);
                }
            }
            (syn::Item::Struct(item), Some(marker)) => match marker.kind {
                attrs::MarkerKind::Record => {
                    interface
                        .records
                        .push(data::lower_record(item, &marker, &declared)?);
                }
                attrs::MarkerKind::Error => {
                    return Err(LowerError::new(
                        marker.span,
                        "#[unibind::error] goes on an enum; each variant becomes \
                         an exception class",
                    ));
                }
                attrs::MarkerKind::Object => return Err(object_unsupported(marker.span)),
            },
            (syn::Item::Enum(item), Some(marker)) => match marker.kind {
                attrs::MarkerKind::Error => {
                    interface.errors.push(data::lower_error(item, &marker)?);
                }
                attrs::MarkerKind::Record => {
                    return Err(LowerError::new(
                        marker.span,
                        "data enums are not part of phase 0; model the value as a \
                         #[unibind::record] struct until enums land",
                    ));
                }
                attrs::MarkerKind::Object => return Err(object_unsupported(marker.span)),
            },
            (_, Some(marker)) => {
                return Err(match marker.kind {
                    attrs::MarkerKind::Object => object_unsupported(marker.span),
                    attrs::MarkerKind::Record | attrs::MarkerKind::Error => LowerError::new(
                        marker.span,
                        "this unibind marker goes on a struct (record) or enum (error)",
                    ),
                });
            }
            // Anything unannotated that is not a pub fn passes through as
            // plain Rust: impls, uses, consts, private helpers.
            _ => {}
        }
    }
    Ok(interface)
}

fn object_unsupported(span: Span) -> LowerError {
    LowerError::new(
        span,
        "#[unibind::object] lands with resources in phase 2 (issue #1992); \
         phase 0 covers sync functions, records, and errors",
    )
}

fn collect_declared(items: &[syn::Item]) -> Result<Declared> {
    let mut declared = Declared::default();
    for item in items {
        let Some(marker) = attrs::marker(item)? else {
            continue;
        };
        match (item, marker.kind) {
            (syn::Item::Struct(item), attrs::MarkerKind::Record) => {
                declared.records.push(item.ident.to_string());
            }
            (syn::Item::Enum(item), attrs::MarkerKind::Error) => {
                declared.errors.push(item.ident.to_string());
            }
            _ => {}
        }
    }
    Ok(declared)
}

/// Remove every `#[unibind...]` attribute from the module's items so the
/// re-emitted Rust compiles without the markers.
pub fn strip_unibind_attrs(module: &mut syn::ItemMod) {
    strip(&mut module.attrs);
    let Some((_, items)) = &mut module.content else {
        return;
    };
    for item in items {
        match item {
            syn::Item::Fn(func) => {
                strip(&mut func.attrs);
                for input in &mut func.sig.inputs {
                    if let syn::FnArg::Typed(arg) = input {
                        strip(&mut arg.attrs);
                    }
                }
            }
            syn::Item::Struct(item) => {
                strip(&mut item.attrs);
                for field in &mut item.fields {
                    strip(&mut field.attrs);
                }
            }
            syn::Item::Enum(item) => {
                strip(&mut item.attrs);
                for variant in &mut item.variants {
                    strip(&mut variant.attrs);
                    for field in &mut variant.fields {
                        strip(&mut field.attrs);
                    }
                }
            }
            _ => {}
        }
    }
}

fn strip(attributes: &mut Vec<syn::Attribute>) {
    attributes.retain(|attribute| !attrs::is_unibind_path(attribute.path()));
}
