//! Lower records, unit enums, and error enums.

use syn::spanned::Spanned as _;

use super::ty::{Position, lower_type};
use super::{Declared, LowerError, Result, attrs, marker};
use crate::casing;
use crate::ir;

pub(super) fn lower_record(
    item: &syn::ItemStruct,
    found: &marker::Marker,
    declared: &Declared,
) -> Result<ir::Record> {
    reject_flags(&found.meta, "a record")?;
    found.meta.reject_default("a record")?;
    found.meta.reject_rename_all("a record")?;
    found.meta.reject_py_base("a record")?;
    found.meta.reject_jvm_base("a record")?;
    require_pub(&item.vis, item.ident.span(), "record")?;
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(LowerError::new(
            item.generics.span(),
            "generic records cannot cross the binding boundary",
        ));
    }
    let syn::Fields::Named(fields) = &item.fields else {
        return Err(LowerError::new(
            item.fields.span(),
            "records need named fields; tuple and unit structs are not part \
             of phase 0",
        ));
    };

    let mut lowered = Vec::new();
    for field in &fields.named {
        let Some(ident) = &field.ident else {
            continue;
        };
        require_pub(&field.vis, ident.span(), "record field")?;
        let meta = attrs::UnibindMeta::from_attrs(&field.attrs)?;
        reject_flags(&meta, "a record field")?;
        meta.reject_default("a record field")?;
        meta.reject_rename_all("a record field")?;
        meta.reject_py_base("a record field")?;
        meta.reject_jvm_base("a record field")?;
        lowered.push(ir::Field {
            name: ident.to_string(),
            names: meta.names(),
            docs: marker::doc_lines(&field.attrs),
            ty: lower_type(&field.ty, declared, Position::Owned)?,
        });
    }
    Ok(ir::Record {
        name: item.ident.to_string(),
        names: found.meta.names(),
        docs: marker::doc_lines(&item.attrs),
        fields: lowered,
    })
}

/// Lower a `#[unibind::enumeration]` enum: a closed set of unit variants.
///
/// The variants' wire spellings are decided here, once, from the Rust names
/// and the enum's `rename_all` -- one string per variant for every language,
/// so a backend renders a type name in its own idiom without also inventing
/// a vocabulary.
pub(super) fn lower_enum(item: &syn::ItemEnum, found: &marker::Marker) -> Result<ir::Enum> {
    reject_flags(&found.meta, "an enumeration")?;
    found.meta.reject_default("an enumeration")?;
    found.meta.reject_py_base("an enumeration")?;
    found.meta.reject_jvm_base("an enumeration")?;
    require_pub(&item.vis, item.ident.span(), "enumeration")?;
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(LowerError::new(
            item.generics.span(),
            "generic enums cannot cross the binding boundary",
        ));
    }
    if item.variants.is_empty() {
        return Err(LowerError::new(
            item.ident.span(),
            "an enumeration needs at least one variant; an empty closed set \
             has no value a caller could ever pass",
        ));
    }
    let casing = found.meta.rename_all.unwrap_or_default();

    let mut variants = Vec::new();
    let mut wires: Vec<String> = Vec::new();
    for variant in &item.variants {
        if !matches!(variant.fields, syn::Fields::Unit) {
            return Err(LowerError::new(
                variant.fields.span(),
                format!(
                    "variant `{}::{}` carries data, and #[unibind::enumeration] \
                     renders a closed set of names only. A variant with fields is \
                     a sum type, which each language renders differently (a \
                     discriminated union, a sealed hierarchy, a tagged tuple); \
                     that is tracked separately and not supported yet. Model the \
                     payload as a #[unibind::record] beside a unit enum until it \
                     lands.",
                    item.ident,
                    variant.ident,
                ),
            ));
        }
        let meta = attrs::UnibindMeta::from_attrs(&variant.attrs)?;
        reject_flags(&meta, "an enumeration variant")?;
        meta.reject_default("an enumeration variant")?;
        meta.reject_rename_all("an enumeration variant")?;
        meta.reject_py_base("an enumeration variant")?;
        meta.reject_jvm_base("an enumeration variant")?;
        let name = variant.ident.to_string();
        let wire = casing.apply(&name);
        // Two variants that collide on the wire would be one value the
        // boundary cannot map back, so the round trip would silently pick
        // whichever arm renders first.
        if wires.contains(&wire) {
            return Err(LowerError::new(
                variant.ident.span(),
                format!(
                    "`{}::{}` spells `{wire}` on the wire, which another variant \
                     already claims; pick a `rename_all` convention that keeps \
                     them distinct",
                    item.ident, variant.ident,
                ),
            ));
        }
        wires.push(wire.clone());
        variants.push(ir::EnumVariant {
            names: py_member_names(&meta, &name),
            name,
            wire,
            docs: marker::doc_lines(&variant.attrs),
        });
    }
    Ok(ir::Enum {
        name: item.ident.to_string(),
        names: found.meta.names(),
        docs: marker::doc_lines(&item.attrs),
        variants,
    })
}

/// A variant's per-language member identifiers. Python spells enum members
/// `SCREAMING_SNAKE_CASE`, so that name is filled in here rather than left
/// for the two Python renderers (the pyo3 backend and the stub emitter) to
/// derive separately and disagree about; `py(name = "...")` overrides it.
fn py_member_names(meta: &attrs::UnibindMeta, variant: &str) -> ir::Names {
    let mut names = meta.names();
    names.py = Some(
        names
            .py
            .unwrap_or_else(|| casing::screaming_snake_case(variant)),
    );
    names
}

pub(super) fn lower_error(item: &syn::ItemEnum, found: &marker::Marker) -> Result<ir::ErrorType> {
    reject_flags(&found.meta, "an error enum")?;
    found.meta.reject_default("an error enum")?;
    found.meta.reject_rename_all("an error enum")?;
    require_pub(&item.vis, item.ident.span(), "error enum")?;
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(LowerError::new(
            item.generics.span(),
            "generic error enums cannot cross the binding boundary",
        ));
    }
    if item.variants.is_empty() {
        return Err(LowerError::new(
            item.ident.span(),
            "an error enum needs at least one variant",
        ));
    }

    let mut variants = Vec::new();
    for variant in &item.variants {
        let meta = attrs::UnibindMeta::from_attrs(&variant.attrs)?;
        reject_flags(&meta, "an error variant")?;
        meta.reject_default("an error variant")?;
        meta.reject_rename_all("an error variant")?;
        meta.reject_py_base("an error variant")?;
        meta.reject_jvm_base("an error variant")?;
        variants.push(ir::ErrorVariant {
            name: variant.ident.to_string(),
            names: meta.names(),
            docs: marker::doc_lines(&variant.attrs),
        });
    }
    Ok(ir::ErrorType {
        name: item.ident.to_string(),
        names: found.meta.names(),
        docs: marker::doc_lines(&item.attrs),
        py_base: found.meta.py_base.clone(),
        jvm_base: found.meta.jvm_base.clone(),
        variants,
    })
}

fn reject_flags(meta: &attrs::UnibindMeta, context: &str) -> Result<()> {
    meta.reject_resource(context)?;
    meta.reject_constructor(context)?;
    meta.reject_blocking(context)?;
    meta.reject_export_options(context)
}

fn require_pub(vis: &syn::Visibility, span: proc_macro2::Span, what: &str) -> Result<()> {
    if matches!(vis, syn::Visibility::Public(_)) {
        Ok(())
    } else {
        Err(LowerError::new(
            span,
            format!("a unibind {what} must be `pub` so the generated glue can reach it"),
        ))
    }
}
