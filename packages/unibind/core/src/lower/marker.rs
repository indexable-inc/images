//! Classify `#[unibind::...]` markers and extract doc comments.

use proc_macro2::{Span, TokenStream};
use syn::spanned::Spanned as _;

use super::attrs::UnibindMeta;
use super::{LowerError, Result};

/// Which marker attribute an item carries.
#[derive(Debug, Clone, Copy)]
pub enum MarkerKind {
    Record,
    Error,
    Object,
}

/// A `#[unibind::record]` / `#[unibind::error]` / `#[unibind::object]`
/// marker found on an item, with its parsed arguments.
#[derive(Debug)]
pub struct Marker {
    pub(crate) kind: MarkerKind,
    pub(crate) span: Span,
    pub(crate) meta: UnibindMeta,
}

/// Classify the unibind marker on an item, parsing its arguments.
pub fn marker(item: &syn::Item) -> Result<Option<Marker>> {
    let attributes = match item {
        syn::Item::Struct(item) => &item.attrs,
        syn::Item::Enum(item) => &item.attrs,
        syn::Item::Fn(item) => &item.attrs,
        _ => return Ok(None),
    };
    let mut found = None;
    for attribute in attributes {
        let Some(kind) = marker_kind(attribute.path()) else {
            continue;
        };
        if found.is_some() {
            return Err(LowerError::new(
                attribute.span(),
                "an item takes at most one unibind marker",
            ));
        }
        let tokens = match &attribute.meta {
            syn::Meta::Path(_) => TokenStream::new(),
            syn::Meta::List(list) => list.tokens.clone(),
            syn::Meta::NameValue(_) => {
                return Err(LowerError::new(
                    attribute.span(),
                    "unibind markers take parenthesized options",
                ));
            }
        };
        found = Some(Marker {
            kind,
            span: attribute.span(),
            meta: UnibindMeta::parse(tokens, attribute.span())?,
        });
    }
    Ok(found)
}

fn marker_kind(path: &syn::Path) -> Option<MarkerKind> {
    let mut segments = path.segments.iter();
    let (first, second, rest) = (segments.next(), segments.next(), segments.next());
    if rest.is_some() || first.is_none_or(|segment| segment.ident != "unibind") {
        return None;
    }
    match second {
        Some(segment) if segment.ident == "record" => Some(MarkerKind::Record),
        Some(segment) if segment.ident == "error" => Some(MarkerKind::Error),
        Some(segment) if segment.ident == "object" => Some(MarkerKind::Object),
        _ => None,
    }
}

/// Whether an attribute path belongs to unibind (`unibind` or
/// `unibind::...`), for stripping.
pub fn is_unibind_path(path: &syn::Path) -> bool {
    path.segments
        .first()
        .is_some_and(|segment| segment.ident == "unibind")
}

/// Extract `///` doc comment lines, trimming the customary leading space.
pub fn doc_lines(attributes: &[syn::Attribute]) -> Vec<String> {
    let mut lines = Vec::new();
    for attribute in attributes {
        if !attribute.path().is_ident("doc") {
            continue;
        }
        let syn::Meta::NameValue(pair) = &attribute.meta else {
            continue;
        };
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(text),
            ..
        }) = &pair.value
        else {
            continue;
        };
        let text = text.value();
        lines.push(text.strip_prefix(' ').unwrap_or(&text).to_owned());
    }
    lines
}
