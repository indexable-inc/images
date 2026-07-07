//! Lower a `#[unibind::export]` module's syn items into [`crate::ir`].

mod attrs;
mod data;
mod func;
mod marker;
mod object;
mod ret;
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

/// A language backend the macro can render, as named in
/// `#[unibind::export(backends(...))]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// The pyo3 backend (`unibind-backend-py`, cargo feature `py`).
    Py,
    /// The napi-rs backend (`unibind-backend-ts`, cargo feature `ts`).
    Ts,
    /// The rustler backend (`unibind-backend-ex`, cargo feature `ex`).
    Ex,
}

/// The backends `#[unibind::export(backends(...))]` selected; `None` when
/// the attribute names none, in which case the macro renders every
/// feature-enabled backend.
///
/// Cargo unifies features across a whole-workspace build, so a workspace
/// holding consumers of different backends compiles the macro with every
/// backend feature on at once. `backends(...)` is how one crate pins its
/// glue to the backends whose runtime dependencies it actually declares.
///
/// # Errors
///
/// Returns a positioned error for malformed `#[unibind::export(...)]`
/// options.
pub fn export_backends(module_args: proc_macro2::TokenStream) -> Result<Option<Vec<Backend>>> {
    let meta = attrs::UnibindMeta::parse(module_args, Span::call_site())?;
    Ok(meta.backends)
}

/// Type names declared in the exported module, used to validate references.
#[derive(Debug, Default)]
pub struct Declared {
    pub records: Vec<String>,
    pub errors: Vec<String>,
    pub objects: Vec<String>,
}

/// Lower an inline `#[unibind::export]` module into an [`ir::Interface`].
///
/// `module_args` is the token stream the attribute itself carried, for
/// example `py(name = "...")` from `#[unibind::export(py(name = "..."))]`.
///
/// # Errors
///
/// Returns a positioned error for anything outside the supported surface:
/// generics, data enums, unsupported boundary types, misplaced markers or
/// flags, or malformed `#[unibind(...)]` metadata.
pub fn lower_module(
    module_args: proc_macro2::TokenStream,
    module: &syn::ItemMod,
) -> Result<ir::Interface> {
    let meta = attrs::UnibindMeta::parse(module_args, module.span())?;
    meta.reject_default("a module")?;
    meta.reject_py_base("a module")?;
    meta.reject_resource("a module")?;
    meta.reject_constructor("a module")?;
    meta.reject_blocking("a module")?;
    let Some((_, items)) = &module.content else {
        return Err(LowerError::new(
            module.span(),
            "#[unibind::export] needs an inline module (`mod name { ... }`); \
             out-of-line modules are not supported",
        ));
    };

    let declared = collect_declared(items)?;
    let mut objects = object::Objects::default();
    let mut interface = ir::Interface {
        version: ir::IR_VERSION,
        name: module.ident.to_string(),
        names: meta.names(),
        docs: marker::doc_lines(&module.attrs),
        functions: Vec::new(),
        records: Vec::new(),
        enums: Vec::new(),
        errors: Vec::new(),
        objects: Vec::new(),
    };

    for item in items {
        match (item, marker::marker(item)?) {
            (syn::Item::Fn(func), None) => {
                if matches!(func.vis, syn::Visibility::Public(_)) {
                    interface.functions.push(func::lower_fn(func, &declared)?);
                }
            }
            (syn::Item::Impl(item), None) => objects.lower_impl(item, &declared)?,
            (syn::Item::Struct(item), Some(found)) => match found.kind {
                marker::MarkerKind::Record => {
                    interface
                        .records
                        .push(data::lower_record(item, &found, &declared)?);
                }
                marker::MarkerKind::Object => objects.declare(item, &found)?,
                marker::MarkerKind::Error => {
                    return Err(LowerError::new(
                        found.span,
                        "#[unibind::error] goes on an enum; each variant becomes \
                         an exception class",
                    ));
                }
            },
            (syn::Item::Enum(item), Some(found)) => match found.kind {
                marker::MarkerKind::Error => {
                    interface.errors.push(data::lower_error(item, &found)?);
                }
                marker::MarkerKind::Record => {
                    return Err(LowerError::new(
                        found.span,
                        "data enums are not part of phase 0; model the value as a \
                         #[unibind::record] struct until enums land",
                    ));
                }
                marker::MarkerKind::Object => return Err(object_misplaced(found.span)),
            },
            (_, Some(found)) => {
                return Err(match found.kind {
                    marker::MarkerKind::Object => object_misplaced(found.span),
                    marker::MarkerKind::Record | marker::MarkerKind::Error => LowerError::new(
                        found.span,
                        "this unibind marker goes on a struct (record) or enum (error)",
                    ),
                });
            }
            // Anything unannotated that is not a pub fn or an impl passes
            // through as plain Rust: uses, consts, private helpers.
            _ => {}
        }
    }
    interface.objects = objects.finish()?;
    Ok(interface)
}

fn object_misplaced(span: Span) -> LowerError {
    LowerError::new(
        span,
        "#[unibind::object] goes on a struct; the handle's state lives in \
         its fields",
    )
}

fn collect_declared(items: &[syn::Item]) -> Result<Declared> {
    let mut declared = Declared::default();
    for item in items {
        let Some(found) = marker::marker(item)? else {
            continue;
        };
        match (item, found.kind) {
            (syn::Item::Struct(item), marker::MarkerKind::Record) => {
                check_fresh(&declared, &item.ident)?;
                declared.records.push(item.ident.to_string());
            }
            (syn::Item::Struct(item), marker::MarkerKind::Object) => {
                check_fresh(&declared, &item.ident)?;
                declared.objects.push(item.ident.to_string());
            }
            (syn::Item::Enum(item), marker::MarkerKind::Error) => {
                check_fresh(&declared, &item.ident)?;
                declared.errors.push(item.ident.to_string());
            }
            _ => {}
        }
    }
    Ok(declared)
}

/// Records, errors, and objects share one type namespace: a reference like
/// `Row` in a signature must resolve to exactly one declaration.
fn check_fresh(declared: &Declared, ident: &syn::Ident) -> Result<()> {
    let name = ident.to_string();
    let taken = declared.records.contains(&name)
        || declared.errors.contains(&name)
        || declared.objects.contains(&name);
    if taken {
        return Err(LowerError::new(
            ident.span(),
            format!("`{name}` is declared twice; records, errors, and objects share one namespace"),
        ));
    }
    Ok(())
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
                strip_fn_args(&mut func.sig);
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
            syn::Item::Impl(item) => {
                strip(&mut item.attrs);
                for impl_item in &mut item.items {
                    if let syn::ImplItem::Fn(method) = impl_item {
                        strip(&mut method.attrs);
                        strip_fn_args(&mut method.sig);
                    }
                }
            }
            _ => {}
        }
    }
}

fn strip_fn_args(signature: &mut syn::Signature) {
    for input in &mut signature.inputs {
        if let syn::FnArg::Typed(arg) = input {
            strip(&mut arg.attrs);
        }
    }
}

fn strip(attributes: &mut Vec<syn::Attribute>) {
    attributes.retain(|attribute| !marker::is_unibind_path(attribute.path()));
}
