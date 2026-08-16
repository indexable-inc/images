//! Lower `#[unibind::object]` structs and their `impl` blocks.

use proc_macro2::Span;
use syn::spanned::Spanned as _;

use super::{Declared, LowerError, Result, attrs, func, marker};
use crate::ir;

/// Objects under construction: struct declarations plus the methods and
/// constructor their `impl` blocks contribute. Blocks may precede the
/// struct in source order, so everything merges in [`Self::finish`].
#[derive(Debug, Default)]
pub(super) struct Objects {
    declarations: Vec<Declaration>,
    impls: Vec<ImplBlock>,
}

#[derive(Debug)]
struct Declaration {
    object: ir::Object,
    /// Points resource-validation errors at the struct.
    span: Span,
}

#[derive(Debug)]
struct ImplBlock {
    name: String,
    /// Points a merge failure at the `impl` block's self type.
    span: Span,
    constructor: Option<SpannedFn>,
    associated: Vec<ir::Function>,
    methods: Vec<ir::Function>,
}

/// A lowered constructor with the span its diagnostics point at.
#[derive(Debug)]
struct SpannedFn {
    function: ir::Function,
    span: Span,
}

impl Objects {
    /// Lower a `#[unibind::object]` struct. The struct itself passes
    /// through untouched (the backend wraps it rather than splicing
    /// attributes), so its fields carry no visibility rules.
    pub(super) fn declare(&mut self, item: &syn::ItemStruct, found: &marker::Marker) -> Result<()> {
        reject_stray_meta(&item.attrs)?;
        found.meta.reject_default("an object")?;
        found.meta.reject_rename_all("an object")?;
        found.meta.reject_py_base("an object")?;
        found.meta.reject_jvm_base("an object")?;
        found.meta.reject_export_options("an object")?;
        found.meta.reject_constructor("an object")?;
        found.meta.reject_blocking("an object")?;
        if !matches!(item.vis, syn::Visibility::Public(_)) {
            return Err(LowerError::new(
                item.ident.span(),
                "a unibind object must be `pub` so the generated glue can reach it",
            ));
        }
        if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
            return Err(LowerError::new(
                item.generics.span(),
                "generic objects cannot cross the binding boundary",
            ));
        }
        self.declarations.push(Declaration {
            object: ir::Object {
                name: item.ident.to_string(),
                names: found.meta.names(),
                docs: marker::doc_lines(&item.attrs),
                resource: found.meta.resource,
                constructor: None,
                associated: Vec::new(),
                methods: Vec::new(),
            },
            span: item.ident.span(),
        });
        Ok(())
    }

    /// Lower one `impl` block from the exported module. Trait impls stay
    /// plain Rust (records and errors need `Display` and friends); an
    /// inherent block must target a declared object.
    pub(super) fn lower_impl(&mut self, item: &syn::ItemImpl, declared: &Declared) -> Result<()> {
        if item.trait_.is_some() {
            return Ok(());
        }
        let name = match impl_target(item) {
            Some(name) if declared.objects.contains(&name) => name,
            _ => {
                return Err(LowerError::new(
                    item.self_ty.span(),
                    "impl blocks inside an exported module belong to \
                     #[unibind::object] types",
                ));
            }
        };
        if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
            return Err(LowerError::new(
                item.generics.span(),
                "generic impl blocks cannot cross the binding boundary",
            ));
        }
        let mut block = ImplBlock {
            name,
            span: item.self_ty.span(),
            constructor: None,
            associated: Vec::new(),
            methods: Vec::new(),
        };
        for impl_item in &item.items {
            // Consts, types, and private helpers stay plain Rust.
            let syn::ImplItem::Fn(method) = impl_item else {
                continue;
            };
            if !matches!(method.vis, syn::Visibility::Public(_)) {
                continue;
            }
            block.lower_method(method, declared)?;
        }
        self.impls.push(block);
        Ok(())
    }

    /// Merge impl blocks into their declarations and validate resources.
    pub(super) fn finish(self) -> Result<Vec<ir::Object>> {
        let Self {
            mut declarations,
            impls,
        } = self;
        for block in impls {
            // lower_impl already refused any block whose target is not a
            // declared object, so this only fires if the pre-scan that fills
            // `Declared` and the struct pass that fills `declarations` ever
            // disagree. A proc macro must answer that with a diagnostic: an
            // unwrap here surfaces to the user as a macro panic with no span.
            let Some(declaration) = declarations
                .iter_mut()
                .find(|declaration| declaration.object.name == block.name)
            else {
                return Err(LowerError::new(
                    block.span,
                    format!(
                        "`{}` has an impl block but no #[unibind::object] \
                         declaration in this module",
                        block.name
                    ),
                ));
            };
            declaration.object.methods.extend(block.methods);
            declaration.object.associated.extend(block.associated);
            if let Some(constructor) = block.constructor {
                if declaration.object.constructor.is_some() {
                    return Err(LowerError::new(
                        constructor.span,
                        "an object takes one constructor",
                    ));
                }
                declaration.object.constructor = Some(constructor.function);
            }
        }
        for declaration in &declarations {
            if declaration.object.resource && !has_close(&declaration.object) {
                return Err(LowerError::new(
                    declaration.span,
                    "a resource needs a close method (zero arguments, no \
                     return value); unibind maps it to close()/aexit and \
                     warns when it never runs",
                ));
            }
        }
        Ok(declarations
            .into_iter()
            .map(|declaration| declaration.object)
            .collect())
    }
}

impl ImplBlock {
    fn lower_method(&mut self, method: &syn::ImplItemFn, declared: &Declared) -> Result<()> {
        if let Some(receiver) = method.sig.receiver() {
            validate_receiver(receiver)?;
            self.methods.push(func::lower_callable(func::Callable {
                attributes: &method.attrs,
                signature: &method.sig,
                declared,
                kind: func::Kind::Method,
            })?);
            return Ok(());
        }
        let meta = attrs::UnibindMeta::from_attrs(&method.attrs)?;
        if meta.associated {
            self.associated.push(func::lower_callable(func::Callable {
                attributes: &method.attrs,
                signature: &method.sig,
                declared,
                kind: func::Kind::Associated { object: &self.name },
            })?);
            return Ok(());
        }
        if !meta.constructor {
            return Err(LowerError::new(
                method.sig.ident.span(),
                "associated functions do not cross the boundary; mark a \
                 constructor with #[unibind(constructor)], a named one on \
                 the type with #[unibind(associated)], or take &self",
            ));
        }
        if self.constructor.is_some() {
            return Err(LowerError::new(
                method.sig.ident.span(),
                "an object takes one constructor",
            ));
        }
        let function = func::lower_callable(func::Callable {
            attributes: &method.attrs,
            signature: &method.sig,
            declared,
            kind: func::Kind::Constructor { object: &self.name },
        })?;
        self.constructor = Some(SpannedFn {
            function,
            span: method.sig.ident.span(),
        });
        Ok(())
    }
}

/// Refuse a bare `#[unibind(...)]` sitting next to an object's marker.
///
/// An object's options come from the marker's own argument list
/// ([`marker::Marker::meta`]); a separate `#[unibind(...)]` attribute is
/// never parsed here, and [`crate::strip_unibind_attrs`] then deletes it
/// from the re-emitted Rust. Spelled `#[unibind(resource)]` -- the natural
/// guess -- that silence costs the whole resource surface: no `close()`
/// requirement, no leak warning, no `async with`, and no diagnostic. Fail
/// loudly instead, naming the spelling that works.
///
/// Only the bare `unibind` path is refused. `#[unibind::object]`,
/// `#[unibind::record]`, and `#[unibind::error]` are markers (two path
/// segments), and the flags that legitimately use the bare form live on
/// methods (`constructor`, `blocking`) and arguments (`default`), neither
/// of which reaches here.
fn reject_stray_meta(attributes: &[syn::Attribute]) -> Result<()> {
    for attribute in attributes {
        if !attribute.path().is_ident("unibind") {
            continue;
        }
        let options = match &attribute.meta {
            syn::Meta::List(list) => list.tokens.to_string(),
            _ => "...".to_owned(),
        };
        return Err(LowerError::new(
            attribute.span(),
            format!(
                "`#[unibind(...)]` next to an object marker is not read; \
                 an object's options belong inside the marker: write \
                 `#[unibind::object({options})]`"
            ),
        ));
    }
    Ok(())
}

/// The bare type name an inherent impl block targets, if it is one.
fn impl_target(item: &syn::ItemImpl) -> Option<String> {
    let syn::Type::Path(path) = &*item.self_ty else {
        return None;
    };
    if path.qself.is_some() {
        return None;
    }
    path.path.get_ident().map(ToString::to_string)
}

fn validate_receiver(receiver: &syn::Receiver) -> Result<()> {
    // `&self` is a plain shared reference: no `mut`, no `self: Ty` form.
    if receiver.reference.is_some()
        && receiver.mutability.is_none()
        && receiver.colon_token.is_none()
    {
        return Ok(());
    }
    Err(LowerError::new(
        receiver.span(),
        "&mut self cannot cross the boundary: Python aliases objects \
         freely; use interior mutability (Mutex, atomics) and take &self",
    ))
}

/// `close` counts with zero arguments and no success value; `Result<(), E>`
/// and async both stay valid teardown shapes.
fn has_close(object: &ir::Object) -> bool {
    object
        .methods
        .iter()
        .any(|method| method.name == "close" && method.args.is_empty() && method.ret.is_none())
}
