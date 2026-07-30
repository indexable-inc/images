//! Lowers a [`Ty`] onto the `__ixTy` checker expression the emitted Nix calls
//! at eval time.
//!
//! Same contract as the value mapper: every type has exactly one runtime
//! lowering. The static half of the type story (tsc against ambient
//! declarations) never runs here; this pass only decides what the emitted Nix
//! asserts, and the `__ixTy` runtime an importer passes decides whether those
//! assertions run (`assert` mode) or vanish (`erase` mode).

use oxc_span::Span;

use crate::error::{LineCol, line_col};
use crate::map::Mapper;
use crate::nix::{Attr, Expr, StrPart};
use crate::ty::{Field, Literal, Ty};

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

/// The checker expression for one type.
pub(crate) fn checker(ty: &Ty) -> Expr {
    match ty {
        Ty::Any => runtime("any"),
        Ty::Str => runtime("string"),
        Ty::Bool => runtime("bool"),
        Ty::Int => runtime("int"),
        Ty::Float => runtime("float"),
        // The runtime names each refined integer's checker after the type, so
        // the table in [`crate::ty`] is the only place the spellings live.
        Ty::Ranged(range) => runtime(range.name),
        Ty::Path => runtime("path"),
        Ty::NonEmptyStr => runtime("nonEmptyStr"),
        Ty::Drv => runtime("drv"),
        Ty::Func => runtime("func"),
        Ty::List(item) => apply(runtime("listOf"), checker(item)),
        Ty::Attrs(value) => apply(runtime("attrsOf"), checker(value)),
        Ty::Object(fields) => apply(
            runtime("attrs"),
            Expr::List(fields.iter().map(field_checker).collect()),
        ),
        Ty::Enum(literals) => apply(
            runtime("enum"),
            Expr::List(literals.iter().map(literal_expr).collect()),
        ),
        Ty::Nullable(inner) => nullable(checker(inner)),
        Ty::Alias(name) => Expr::Ident(alias_binding(name)),
    }
}

/// `__ixTy.req "a" <ty>` / `__ixTy.opt "b" <ty>`: one field of an object type.
fn field_checker(field: &Field) -> Expr {
    // Spelled as two `runtime` calls rather than one on a conditional name so
    // `every_emitted_runtime_field_exists_in_ix_ty_nix` can see both literals.
    let entry = if field.optional {
        runtime("opt")
    } else {
        runtime("req")
    };
    apply(
        apply(entry, str_lit(field.name.clone())),
        checker(&field.ty),
    )
}

fn literal_expr(literal: &Literal) -> Expr {
    match literal {
        Literal::Str(text) => str_lit(text.clone()),
        Literal::Int(value) => Expr::Int(*value),
        Literal::Float(value) => Expr::Float(*value),
        Literal::Bool(value) => Expr::Ident(if *value { "true" } else { "false" }.into()),
    }
}

/// Wraps a checker as `__ixTy.nullable <ty>`.
fn nullable(ty: Expr) -> Expr {
    apply(runtime("nullable"), ty)
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
    pub(crate) fn ret_check(&self, span: Span, what: &str, ty: &Ty, value: Expr) -> Expr {
        apply(
            apply(
                apply(runtime("ret"), str_lit(self.check_loc(span, what))),
                checker(ty),
            ),
            value,
        )
    }
}

#[cfg(test)]
mod tests {
    use crate::ty::INT_RANGES;

    /// Every `__ixTy.<field>` the converter can emit must be defined by the
    /// runtime, or a typed module fails at eval with `attribute missing`
    /// instead of a type error. Scans this crate's emitting source for string
    /// literals passed to the `runtime` spelling helper, plus the two entry
    /// points and the refined-integer names (which reach `runtime` as
    /// [`crate::ty::IntRange`] fields, not as literals), and checks each name
    /// is bound in `ix-ty.nix`.
    #[test]
    fn every_emitted_runtime_field_exists_in_ix_ty_nix() {
        let sources = [include_str!("checker.rs"), include_str!("map.rs")];
        let runtime_nix = include_str!("../ix-ty.nix");
        let mut fields = vec!["arg".to_owned(), "ret".to_owned()];
        fields.extend(INT_RANGES.iter().map(|range| range.name.to_owned()));
        for source in sources {
            for (index, _) in source.match_indices("runtime(\"") {
                let rest = &source[index + 9..];
                let end = rest.find('\"').expect("string literal closes");
                fields.push(rest[..end].to_owned());
            }
        }
        // A scan that matched nothing would pass every assertion below, so the
        // count is pinned too: 2 entry points, 7 refined integers, and the 18
        // literal spellings passed to the helper above.
        assert_eq!(fields.len(), 27, "the runtime-field scan found {fields:?}");
        for field in fields {
            assert!(
                runtime_nix.contains(&format!("{field} =")),
                "__ixTy.{field} is emitted but not defined in ix-ty.nix"
            );
        }
    }
}
