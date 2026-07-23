//! Lowers TypeScript type annotations onto `__ixTy` checker expressions.
//!
//! Same contract as the value mapper: every type either has exactly one
//! runtime lowering or is a positioned [`Error`]. The static half of the
//! type story (tsc against ambient declarations) never runs here; this pass
//! only decides what the emitted Nix asserts at eval time, and the `__ixTy`
//! runtime an importer passes decides whether those assertions run
//! (`assert` mode) or vanish (`erase` mode).

use oxc_ast::ast;
use oxc_span::{GetSpan as _, Span};

use crate::error::{Error, LineCol, line_col};
use crate::map::Mapper;
use crate::nix::{Attr, Expr, StrPart};

/// Spells `__ixTy.<field>`, the only way emitted code reaches the runtime.
fn runtime(field: &str) -> Expr {
    Expr::Select {
        base: Box::new(Expr::Ident("__ixTy".into())),
        path: vec![Attr::Name(field.into())],
        or_default: None,
    }
}

fn apply(function: Expr, argument: Expr) -> Expr {
    Expr::Apply {
        function: Box::new(function),
        argument: Box::new(argument),
    }
}

fn str_lit(text: String) -> Expr {
    Expr::Str(vec![StrPart::Lit(text)])
}

/// `__ixTy.arg "<loc>" <ty> <value> <body>`: checks `value`, then is `body`.
/// Parameter checks wrap the innermost body, where every curried parameter
/// is in scope.
pub(crate) fn arg_check(loc: String, ty: Expr, value: Expr, body: Expr) -> Expr {
    apply(
        apply(apply(apply(runtime("arg"), str_lit(loc)), ty), value),
        body,
    )
}

/// Type names with a fixed meaning; `type` aliases may not shadow them
/// ([`crate::map`] rejects the declaration), so a reference is never
/// ambiguous between a built-in and a module alias.
pub(crate) const BUILTIN_TYPES: [&str; 14] = [
    "bool",
    "int",
    "float",
    "u8",
    "u16",
    "u32",
    "i8",
    "i16",
    "i32",
    "port",
    "path",
    "nonEmptyStr",
    "drv",
    "Record",
];

/// Wraps a checker as `__ixTy.nullable <ty>`; optional pattern fields check
/// against `T | null` because their Nix default binds when the field is
/// absent, and `null` is the conventional "absent" default.
pub(crate) fn nullable(ty: Expr) -> Expr {
    apply(runtime("nullable"), ty)
}

/// The `let` binding carrying a `type X = ...` alias checker. The `'` is
/// valid in Nix identifiers but not JavaScript ones, so alias bindings can
/// never collide with `const` bindings.
pub(crate) fn alias_binding(name: &str) -> String {
    format!("ty'{name}")
}

impl Mapper<'_> {
    /// `"<line>:<col> <what>"`: the source location a failed check reports.
    /// The runtime prefixes the module path, which only the importer knows.
    pub(crate) fn check_loc(&self, span: Span, what: &str) -> String {
        let LineCol { line, column } = line_col(span.start as usize, self.source);
        format!("{line}:{column} {what}")
    }

    /// `__ixTy.ret "<loc>" <ty> <value>`: checks `value`, then is `value`.
    pub(crate) fn ret_check(&self, span: Span, what: &str, ty: Expr, value: Expr) -> Expr {
        apply(
            apply(apply(runtime("ret"), str_lit(self.check_loc(span, what))), ty),
            value,
        )
    }

    /// Lowers one type to its `__ixTy` checker expression.
    pub(crate) fn ty(&self, t: &ast::TSType<'_>) -> Result<Expr, Error> {
        match t {
            ast::TSType::TSAnyKeyword(_) | ast::TSType::TSUnknownKeyword(_) => Ok(runtime("any")),
            ast::TSType::TSStringKeyword(_) => Ok(runtime("string")),
            ast::TSType::TSBooleanKeyword(_) => Ok(runtime("bool")),
            // `object` is any attrset: `attrsOf any`, one spelling for both.
            ast::TSType::TSObjectKeyword(_) => Ok(apply(runtime("attrsOf"), runtime("any"))),
            ast::TSType::TSNumberKeyword(number) => Err(self.err(
                number.span,
                "Nix distinguishes integers from floats; use `int` or `float`",
            )),
            ast::TSType::TSNullKeyword(null) => Err(self.err(
                null.span,
                "`null` is only checkable in a union: `T | null`",
            )),
            ast::TSType::TSUndefinedKeyword(undefined) => Err(self.err(
                undefined.span,
                "`undefined` has no Nix equivalent; use `T | null`",
            )),
            ast::TSType::TSArrayType(array) => {
                Ok(apply(runtime("listOf"), self.ty(&array.element_type)?))
            }
            // Parameter and return types of a function value cannot be probed
            // without calling it; `func` checks callability only.
            ast::TSType::TSFunctionType(_) => Ok(runtime("func")),
            ast::TSType::TSParenthesizedType(paren) => self.ty(&paren.type_annotation),
            ast::TSType::TSTypeLiteral(literal) => self.type_literal(literal),
            ast::TSType::TSUnionType(union) => self.union(union),
            ast::TSType::TSLiteralType(literal) => {
                Ok(apply(runtime("enum"), Expr::List(vec![self.ty_literal(literal)?])))
            }
            ast::TSType::TSTypeReference(reference) => self.reference(reference),
            other => Err(self.err(
                other.span(),
                "this type has no runtime lowering; see the ix2nix type table",
            )),
        }
    }

    /// `{ a: T; b?: U }` lowers to `__ixTy.attrs [ (__ixTy.req "a" tyT) ... ]`.
    fn type_literal(&self, literal: &ast::TSTypeLiteral<'_>) -> Result<Expr, Error> {
        let mut fields = Vec::new();
        for member in &literal.members {
            let ast::TSSignature::TSPropertySignature(property) = member else {
                return Err(self.err(
                    member.span(),
                    "only property signatures lower; index, call, and method signatures have no runtime check",
                ));
            };
            let Some(annotation) = &property.type_annotation else {
                return Err(self.err(property.span, "property signature needs a type"));
            };
            if property.computed {
                return Err(self.err(property.span, "computed keys have no runtime check"));
            }
            let name = match &property.key {
                ast::PropertyKey::StaticIdentifier(ident) => ident.name.to_string(),
                ast::PropertyKey::StringLiteral(lit) => lit.value.to_string(),
                other => {
                    return Err(self.err(other.span(), "property key must be a name or string"));
                }
            };
            let field = if property.optional { "opt" } else { "req" };
            fields.push(apply(
                apply(runtime(field), str_lit(name)),
                self.ty(&annotation.type_annotation)?,
            ));
        }
        Ok(apply(runtime("attrs"), Expr::List(fields)))
    }

    /// Unions lower in exactly two shapes: `T | null` and literal enums
    /// (optionally `| null`). Anything else has no single runtime check.
    fn union(&self, union: &ast::TSUnionType<'_>) -> Result<Expr, Error> {
        let mut nullable = false;
        let mut literals = Vec::new();
        let mut others = Vec::new();
        for member in &union.types {
            match member {
                ast::TSType::TSNullKeyword(_) => nullable = true,
                ast::TSType::TSLiteralType(literal) => literals.push(self.ty_literal(literal)?),
                other => others.push(other),
            }
        }
        let inner = match (literals.is_empty(), others.as_slice()) {
            (false, []) => apply(runtime("enum"), Expr::List(literals)),
            (true, [single]) => self.ty(single)?,
            _ => {
                return Err(self.err(
                    union.span,
                    "unions lower only as `T | null` or a union of literals",
                ));
            }
        };
        Ok(if nullable {
            apply(runtime("nullable"), inner)
        } else {
            inner
        })
    }

    fn ty_literal(&self, literal: &ast::TSLiteralType<'_>) -> Result<Expr, Error> {
        match &literal.literal {
            ast::TSLiteral::StringLiteral(lit) => Ok(str_lit(lit.value.to_string())),
            ast::TSLiteral::NumericLiteral(lit) => self.number(lit),
            ast::TSLiteral::BooleanLiteral(lit) => {
                Ok(Expr::Ident(if lit.value { "true" } else { "false" }.into()))
            }
            other => Err(self.err(other.span(), "this literal type has no runtime check")),
        }
    }

    /// Named types: the built-ins (`Int`, `Float`, `Drv`, `Record<string, T>`)
    /// and in-module `type` aliases. Everything else is unknown here; ambient
    /// declaration files are tsc's world, not the converter's.
    fn reference(&self, reference: &ast::TSTypeReference<'_>) -> Result<Expr, Error> {
        let ast::TSTypeName::IdentifierReference(ident) = &reference.type_name else {
            return Err(self.err(
                reference.span,
                "qualified type names have no runtime lowering",
            ));
        };
        let name = ident.name.as_str();
        if name == "Record" {
            let Some(arguments) = &reference.type_arguments else {
                return Err(self.err(reference.span, "`Record` needs `<string, T>` arguments"));
            };
            let [key, value] = arguments.params.as_slice() else {
                return Err(self.err(arguments.span, "`Record` takes exactly two arguments"));
            };
            if !matches!(key, ast::TSType::TSStringKeyword(_)) {
                return Err(self.err(
                    key.span(),
                    "Nix attrset keys are strings; use `Record<string, T>`",
                ));
            }
            return Ok(apply(runtime("attrsOf"), self.ty(value)?));
        }
        if let Some(arguments) = &reference.type_arguments {
            return Err(self.err(arguments.span, "generic types are not lowered yet"));
        }
        // Lowercase on purpose: `int` and `float` are what Nix itself calls
        // the types, the width refinements are Rust vocabulary, and `port` /
        // `path` / `nonEmptyStr` come from nixpkgs `lib.types`. TypeScript's
        // bare `number` stays banned.
        match name {
            // TypeScript's keyword is `boolean`; Nix's name is `bool`. The
            // keyword arrives as TSBooleanKeyword, this arm catches the Nix
            // spelling, and both lower to the same checker.
            "bool" => Ok(runtime("bool")),
            "int" => Ok(runtime("int")),
            "float" => Ok(runtime("float")),
            "u8" => Ok(runtime("u8")),
            "u16" => Ok(runtime("u16")),
            "u32" => Ok(runtime("u32")),
            "i8" => Ok(runtime("i8")),
            "i16" => Ok(runtime("i16")),
            "i32" => Ok(runtime("i32")),
            "port" => Ok(runtime("port")),
            "path" => Ok(runtime("path")),
            "nonEmptyStr" => Ok(runtime("nonEmptyStr")),
            "drv" => Ok(runtime("drv")),
            _ if self.type_aliases.contains(name) => Ok(Expr::Ident(alias_binding(name))),
            _ => Err(self.err(
                ident.span,
                format!(
                    "unknown type `{name}`; built-ins are int, float, \
                     u8/u16/u32/i8/i16/i32, port, path, nonEmptyStr, drv, and \
                     Record<string, T>, plus this module's `type` aliases"
                ),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    /// Every `__ixTy.<field>` the converter can emit must be defined by the
    /// runtime, or a typed module fails at eval with `attribute missing`
    /// instead of a type error. Scans this crate's two emitting sources for
    /// string literals passed to the `runtime` spelling helper, plus the two
    /// entry points, and checks each name is bound in `ix-ty.nix`.
    #[test]
    fn every_emitted_runtime_field_exists_in_ix_ty_nix() {
        let sources = [include_str!("ty.rs"), include_str!("map.rs")];
        let runtime_nix = include_str!("../ix-ty.nix");
        let mut fields = vec!["arg".to_owned(), "ret".to_owned()];
        for source in sources {
            for (index, _) in source.match_indices("runtime(\"") {
                let rest = &source[index + 9..];
                let end = rest.find('\"').expect("string literal closes");
                fields.push(rest[..end].to_owned());
            }
        }
        for field in fields {
            assert!(
                runtime_nix.contains(&format!("{field} =")),
                "__ixTy.{field} is emitted but not defined in ix-ty.nix"
            );
        }
    }
}
