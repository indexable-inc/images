//! Parse `#[unibind(...)]` metadata options.

use proc_macro2::{Span, TokenStream};
use syn::spanned::Spanned as _;

use super::{Backend, LowerError, Result};
use crate::ir;

/// The options a `#[unibind(...)]` attribute (or marker argument list) can
/// carry: `py(name = "...")`, `py(base = "...")`, `ts(name = "...")`,
/// `default = ...`, the bare flags `resource`, `constructor`, and
/// `blocking`, and (on `#[unibind::export]` only) `backends(...)`.
#[derive(Debug, Default)]
pub struct UnibindMeta {
    pub(crate) span: Option<Span>,
    pub(crate) py_name: Option<String>,
    pub(crate) py_base: Option<String>,
    pub(crate) ts_name: Option<String>,
    pub(crate) default: Option<ir::Literal>,
    pub(crate) resource: bool,
    pub(crate) constructor: bool,
    pub(crate) blocking: bool,
    pub(crate) backends: Option<Vec<Backend>>,
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
        if other.ts_name.is_some() {
            if self.ts_name.is_some() {
                return Err(LowerError::new(span, "duplicate unibind `ts(name = ...)`"));
            }
            self.ts_name = other.ts_name;
        }
        if other.default.is_some() {
            if self.default.is_some() {
                return Err(LowerError::new(span, "duplicate unibind `default`"));
            }
            self.default = other.default;
        }
        if other.backends.is_some() {
            if self.backends.is_some() {
                return Err(LowerError::new(span, "duplicate unibind `backends(...)`"));
            }
            self.backends = other.backends;
        }
        if other.resource {
            if self.resource {
                return Err(LowerError::new(span, "duplicate unibind `resource`"));
            }
            self.resource = true;
        }
        if other.constructor {
            if self.constructor {
                return Err(LowerError::new(span, "duplicate unibind `constructor`"));
            }
            self.constructor = true;
        }
        if other.blocking {
            if self.blocking {
                return Err(LowerError::new(span, "duplicate unibind `blocking`"));
            }
            self.blocking = true;
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
        if entry.path().is_ident("ts") {
            let syn::Meta::List(list) = entry else {
                return Err(LowerError::new(span, "`ts` takes a list: ts(name = \"...\")"));
            };
            let parser =
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated;
            let entries = syn::parse::Parser::parse2(parser, list.tokens.clone())
                .map_err(|error| LowerError::new(span, format!("bad `ts` options: {error}")))?;
            for nested in entries {
                self.apply_ts(&nested)?;
            }
            return Ok(());
        }
        if entry.path().is_ident("backends") {
            return self.apply_backends(entry, span);
        }
        if entry.path().is_ident("default") {
            let syn::Meta::NameValue(pair) = entry else {
                return Err(LowerError::new(span, "`default` takes a value: default = ..."));
            };
            self.default = Some(literal(&pair.value)?);
            return Ok(());
        }
        if let syn::Meta::Path(path) = entry {
            let flag = if path.is_ident("resource") {
                &mut self.resource
            } else if path.is_ident("constructor") {
                &mut self.constructor
            } else if path.is_ident("blocking") {
                &mut self.blocking
            } else {
                return Err(unknown_option(span));
            };
            if *flag {
                return Err(LowerError::new(span, "duplicate unibind flag"));
            }
            *flag = true;
            return Ok(());
        }
        Err(unknown_option(span))
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

    /// Parse `ts(name = "...")`: the TypeScript-side rename.
    fn apply_ts(&mut self, entry: &syn::Meta) -> Result<()> {
        let span = entry.span();
        let syn::Meta::NameValue(pair) = entry else {
            return Err(LowerError::new(span, "the `ts` option is name = \"...\""));
        };
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(value),
            ..
        }) = &pair.value
        else {
            return Err(LowerError::new(span, "`ts` options take string literals"));
        };
        if pair.path.is_ident("name") {
            self.ts_name = Some(value.value());
        } else {
            return Err(LowerError::new(
                span,
                "unknown `ts` option; expected name = \"...\"",
            ));
        }
        Ok(())
    }

    /// Parse `backends(py, ts)`: which enabled backends an export renders.
    fn apply_backends(&mut self, entry: &syn::Meta, span: Span) -> Result<()> {
        let syn::Meta::List(list) = entry else {
            return Err(LowerError::new(span, "`backends` takes a list: backends(py, ts)"));
        };
        let parser = syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated;
        let entries = syn::parse::Parser::parse2(parser, list.tokens.clone())
            .map_err(|error| LowerError::new(span, format!("bad `backends` list: {error}")))?;
        let mut backends = Vec::new();
        for path in &entries {
            let backend = if path.is_ident("py") {
                Backend::Py
            } else if path.is_ident("ts") {
                Backend::Ts
            } else {
                return Err(LowerError::new(path.span(), "unknown backend; expected `py` or `ts`"));
            };
            if backends.contains(&backend) {
                return Err(LowerError::new(path.span(), "duplicate backend"));
            }
            backends.push(backend);
        }
        if backends.is_empty() {
            return Err(LowerError::new(span, "`backends(...)` names at least one backend"));
        }
        self.backends = Some(backends);
        Ok(())
    }

    pub(crate) fn names(&self) -> ir::Names {
        ir::Names {
            py: self.py_name.clone(),
            ts: self.ts_name.clone(),
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

    /// Error out when a `resource` flag was given somewhere it cannot apply.
    pub(crate) fn reject_resource(&self, context: &str) -> Result<()> {
        if self.resource {
            return Err(LowerError::new(
                self.span.unwrap_or_else(Span::call_site),
                format!("`resource` applies to #[unibind::object] markers, not {context}"),
            ));
        }
        Ok(())
    }

    /// Error out when a `constructor` flag was given somewhere it cannot
    /// apply.
    pub(crate) fn reject_constructor(&self, context: &str) -> Result<()> {
        if self.constructor {
            return Err(LowerError::new(
                self.span.unwrap_or_else(Span::call_site),
                format!(
                    "`constructor` applies to associated functions in an \
                     object impl block, not {context}"
                ),
            ));
        }
        Ok(())
    }

    /// Error out when a `blocking` flag was given somewhere it cannot apply.
    pub(crate) fn reject_blocking(&self, context: &str) -> Result<()> {
        if self.blocking {
            return Err(LowerError::new(
                self.span.unwrap_or_else(Span::call_site),
                format!("`blocking` applies to exported functions and object methods, not {context}"),
            ));
        }
        Ok(())
    }

    /// Error out when a `backends(...)` was given somewhere it cannot apply.
    pub(crate) fn reject_backends(&self, context: &str) -> Result<()> {
        if self.backends.is_some() {
            return Err(LowerError::new(
                self.span.unwrap_or_else(Span::call_site),
                format!("`backends(...)` applies to #[unibind::export], not {context}"),
            ));
        }
        Ok(())
    }
}

fn unknown_option(span: Span) -> LowerError {
    LowerError::new(
        span,
        "unknown unibind option; expected py(name = \"...\"), \
         py(base = \"...\"), ts(name = \"...\"), backends(...), \
         default = ..., resource, constructor, or blocking",
    )
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
