//! Lower `#[unibind::object]` structs and their `impl` blocks.

use proc_macro2::Span;
use syn::spanned::Spanned as _;

use super::{attrs, func, marker, Declared, LowerError, Result};
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
    constructor: Option<SpannedFn>,
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
        found.meta.reject_default("an object")?;
        found.meta.reject_py_base("an object")?;
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
            constructor: None,
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
            let declaration = declarations
                .iter_mut()
                .find(|declaration| declaration.object.name == block.name)
                .expect("impl targets were validated against declared objects");
            declaration.object.methods.extend(block.methods);
            if let Some(constructor) = block.constructor {
                if declaration.object.constructor.is_some() {
                    return Err(LowerError::new(constructor.span, "an object takes one constructor"));
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
            self.methods.push(func::lower_callable(
                &method.attrs,
                &method.sig,
                declared,
                func::Kind::Method,
            )?);
            return Ok(());
        }
        let meta = attrs::UnibindMeta::from_attrs(&method.attrs)?;
        if !meta.constructor {
            return Err(LowerError::new(
                method.sig.ident.span(),
                "associated functions do not cross the boundary; mark the \
                 constructor with #[unibind(constructor)] or take &self",
            ));
        }
        if self.constructor.is_some() {
            return Err(LowerError::new(
                method.sig.ident.span(),
                "an object takes one constructor",
            ));
        }
        let function = func::lower_callable(
            &method.attrs,
            &method.sig,
            declared,
            func::Kind::Constructor { object: &self.name },
        )?;
        self.constructor = Some(SpannedFn {
            function,
            span: method.sig.ident.span(),
        });
        Ok(())
    }
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
    if receiver.reference.is_some() && receiver.mutability.is_none() && receiver.colon_token.is_none()
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
