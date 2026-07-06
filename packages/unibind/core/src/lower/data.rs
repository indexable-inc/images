//! Lower records and error enums.

use syn::spanned::Spanned as _;

use super::ty::{lower_type, Position};
use super::{attrs, Declared, LowerError, Result};
use crate::ir;

pub(super) fn lower_record(
    item: &syn::ItemStruct,
    marker: &attrs::Marker,
    declared: &Declared,
) -> Result<ir::Record> {
    marker.meta.reject_default("a record")?;
    marker.meta.reject_py_base("a record")?;
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
        meta.reject_default("a record field")?;
        meta.reject_py_base("a record field")?;
        lowered.push(ir::Field {
            name: ident.to_string(),
            names: meta.names(),
            docs: attrs::doc_lines(&field.attrs),
            ty: lower_type(&field.ty, declared, Position::Owned)?,
        });
    }
    Ok(ir::Record {
        name: item.ident.to_string(),
        names: marker.meta.names(),
        docs: attrs::doc_lines(&item.attrs),
        fields: lowered,
    })
}

pub(super) fn lower_error(item: &syn::ItemEnum, marker: &attrs::Marker) -> Result<ir::ErrorType> {
    marker.meta.reject_default("an error enum")?;
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
        meta.reject_default("an error variant")?;
        meta.reject_py_base("an error variant")?;
        variants.push(ir::ErrorVariant {
            name: variant.ident.to_string(),
            names: meta.names(),
            docs: attrs::doc_lines(&variant.attrs),
        });
    }
    Ok(ir::ErrorType {
        name: item.ident.to_string(),
        names: marker.meta.names(),
        docs: attrs::doc_lines(&item.attrs),
        py_base: marker.meta.py_base.clone(),
        variants,
    })
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
