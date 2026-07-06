//! Parse `#[unibind(...)]` metadata and `#[unibind::...]` markers.

use proc_macro2::{Span, TokenStream};
use syn::spanned::Spanned as _;

use super::{LowerError, Result};
use crate::ir;

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

/// The options a `#[unibind(...)]` attribute (or marker argument list) can
/// carry: `py(name = "...")`, `py(base = "...")`, and `default = ...`.
#[derive(Debug, Default)]
pub struct UnibindMeta {
    pub(crate) span: Option<Span>,
    pub(crate) py_name: Option<String>,
    pub(crate) py_base: Option<String>,
    pub(crate) default: Option<ir::Literal>,
}

impl UnibindMeta {
    /// Parse one argument token stream, as carried by the attribute itself.
    pub(crate) fn parse(tokens: TokenStream, span: Span) -> Result<Self> {
        let mut meta = Self {
            span: Some(span),
            ..Self::default()
        };
        if tokens.is_empty() {
            return Ok(meta);
        }
        let parser = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;
        let entries = syn::parse::Parser::parse2(parser, tokens)
            .map_err(|error| LowerError::new(span, format!("bad unibind options: {error}")))?;
        for entry in entries {
            meta.apply(&entry)?;
        }
        Ok(meta)
    }

    /// Parse and merge every `#[unibind(...)]` attribute in `attributes`.
    pub(crate) fn from_attrs(attributes: &[syn::Attribute]) -> Result<Self> {
        let mut merged = Self::default();
        for attribute in attributes {
            if !attribute.path().is_ident("unibind") {
                continue;
            }
            let syn::Meta::List(list) = &attribute.meta else {
                return Err(LowerError::new(
                    attribute.span(),
                    "#[unibind] takes options: #[unibind(py(name = \"...\"))] or \
                     #[unibind(default = ...)]",
                ));
            };
            let parsed = Self::parse(list.tokens.clone(), attribute.span())?;
            merged.merge(parsed, attribute.span())?;
        }
        Ok(merged)
    }

    fn merge(&mut self, other: Self, span: Span) -> Result<()> {
        if other.py_name.is_some() {
            if self.py_name.is_some() {
                return Err(LowerError::new(span, "duplicate unibind `py(name = ...)`"));
            }
            self.py_name = other.py_name;
        }
        if other.py_base.is_some() {
            if self.py_base.is_some() {
                return Err(LowerError::new(span, "duplicate unibind `py(base = ...)`"));
            }
            self.py_base = other.py_base;
        }
        if other.default.is_some() {
            if self.default.is_some() {
                return Err(LowerError::new(span, "duplicate unibind `default`"));
            }
            self.default = other.default;
        }
        self.span = self.span.or(Some(span));
        Ok(())
    }

    fn apply(&mut self, entry: &syn::Meta) -> Result<()> {
        let span = entry.span();
        if entry.path().is_ident("py") {
            let syn::Meta::List(list) = entry else {
                return Err(LowerError::new(
                    span,
                    "`py` takes a list: py(name = \"...\") or py(base = \"...\")",
                ));
            };
            let parser =
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;
            let entries = syn::parse::Parser::parse2(parser, list.tokens.clone())
                .map_err(|error| LowerError::new(span, format!("bad `py` options: {error}")))?;
            for nested in entries {
                self.apply_py(&nested)?;
            }
            return Ok(());
        }
        if entry.path().is_ident("default") {
            let syn::Meta::NameValue(pair) = entry else {
                return Err(LowerError::new(span, "`default` takes a value: default = ..."));
            };
            self.default = Some(literal(&pair.value)?);
            return Ok(());
        }
        Err(LowerError::new(
            span,
            "unknown unibind option; expected py(name = \"...\"), \
             py(base = \"...\"), or default = ...",
        ))
    }

    fn apply_py(&mut self, entry: &syn::Meta) -> Result<()> {
        let span = entry.span();
        let syn::Meta::NameValue(pair) = entry else {
            return Err(LowerError::new(
                span,
                "`py` options are name = \"...\" and base = \"...\"",
            ));
        };
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(value),
            ..
        }) = &pair.value
        else {
            return Err(LowerError::new(span, "`py` options take string literals"));
        };
        if pair.path.is_ident("name") {
            self.py_name = Some(value.value());
        } else if pair.path.is_ident("base") {
            self.py_base = Some(value.value());
        } else {
            return Err(LowerError::new(
                span,
                "unknown `py` option; expected name = \"...\" or base = \"...\"",
            ));
        }
        Ok(())
    }

    pub(crate) fn names(&self) -> ir::Names {
        ir::Names {
            py: self.py_name.clone(),
        }
    }

    /// Error out when a `default` was given somewhere it cannot apply.
    pub(crate) fn reject_default(&self, context: &str) -> Result<()> {
        if self.default.is_some() {
            return Err(LowerError::new(
                self.span.unwrap_or_else(Span::call_site),
                format!("`default` applies to function arguments, not {context}"),
            ));
        }
        Ok(())
    }

    /// Error out when a `py(base = ...)` was given somewhere it cannot apply.
    pub(crate) fn reject_py_base(&self, context: &str) -> Result<()> {
        if self.py_base.is_some() {
            return Err(LowerError::new(
                self.span.unwrap_or_else(Span::call_site),
                format!("`py(base = ...)` applies to #[unibind::error] enums, not {context}"),
            ));
        }
        Ok(())
    }
}

fn literal(expr: &syn::Expr) -> Result<ir::Literal> {
    let span = expr.span();
    match expr {
        syn::Expr::Lit(lit) => literal_from_lit(&lit.lit),
        syn::Expr::Path(path) if path.path.is_ident("None") => Ok(ir::Literal::None),
        syn::Expr::Unary(syn::ExprUnary {
            op: syn::UnOp::Neg(_),
            expr,
            ..
        }) => match literal(expr)? {
            ir::Literal::Int(value) => Ok(ir::Literal::Int(-value)),
            ir::Literal::Float(value) => Ok(ir::Literal::Float(-value)),
            _ => Err(LowerError::new(span, "only numbers can be negated")),
        },
        _ => Err(LowerError::new(
            span,
            "`default` takes a literal (bool, int, float, string) or None",
        )),
    }
}

fn literal_from_lit(lit: &syn::Lit) -> Result<ir::Literal> {
    match lit {
        syn::Lit::Bool(value) => Ok(ir::Literal::Bool(value.value())),
        syn::Lit::Int(value) => value
            .base10_parse()
            .map(ir::Literal::Int)
            .map_err(|error| LowerError::new(lit.span(), format!("bad integer default: {error}"))),
        syn::Lit::Float(value) => value
            .base10_parse()
            .map(ir::Literal::Float)
            .map_err(|error| LowerError::new(lit.span(), format!("bad float default: {error}"))),
        syn::Lit::Str(value) => Ok(ir::Literal::Str(value.value())),
        _ => Err(LowerError::new(
            lit.span(),
            "`default` takes a literal (bool, int, float, string) or None",
        )),
    }
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
