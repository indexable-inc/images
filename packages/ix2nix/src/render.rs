//! The one place Nix syntax is emitted.
//!
//! Renders the typed tree from [`crate::nix`] with precedence-aware
//! parenthesization, so output is always grammatically unambiguous and
//! byte-for-byte deterministic. Both halves are enforced rather than asserted:
//! `well_formed_programs_convert_and_emitted_nix_reparses` reparses every
//! generated module under rnix, and `convert_is_deterministic` converts each
//! twice.

use std::fmt::Write as _;

use crate::nix::{Attr, BinaryOp, Binding, Expr, Module, Param, StrPart, UnaryOp, is_bare_ident};

/// Binding strength, derived from the Nix operator table (higher binds
/// tighter). Atoms sit above everything; lambdas, `let`, and `if` sit at the
/// bottom.
const ATOM: u8 = 16;
const SELECT: u8 = 15;
const APPLY: u8 = 14;
const NEG: u8 = 13;
const CONCAT: u8 = 11;
const MUL: u8 = 10;
const ADD: u8 = 9;
const NOT: u8 = 8;
const UPDATE: u8 = 7;
const CMP: u8 = 6;
const EQUALITY: u8 = 5;
const AND: u8 = 4;
const OR: u8 = 3;
const LOWEST: u8 = 0;

/// Rendering context: the loosest binding strength the position tolerates
/// without parentheses, and the current indentation depth.
#[derive(Clone, Copy)]
struct Ctx {
    min: u8,
    indent: usize,
}

impl Ctx {
    const fn with_min(self, min: u8) -> Self {
        Self {
            min,
            indent: self.indent,
        }
    }

    const fn nested(self, min: u8) -> Self {
        Self {
            min,
            indent: self.indent + 1,
        }
    }
}

/// Renders a whole module under the `{ __dir, __importIx, __ixTy }:` wrapper, so every
/// converted module presents one calling convention to its importer.
#[must_use]
pub fn module(module: &Module) -> String {
    let mut out = String::new();
    out.push_str("{ __dir, __importIx, __ixTy }:\n");
    expr(
        &mut out,
        &module.body,
        Ctx {
            min: LOWEST,
            indent: 0,
        },
    );
    out.push('\n');
    out
}

/// How tightly `e` binds, i.e. the loosest context it can appear in without
/// parentheses.
const fn strength(e: &Expr) -> u8 {
    match e {
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Path(_)
        | Expr::Ident(_)
        | Expr::AttrSet(_)
        | Expr::List(_) => ATOM,
        Expr::Select { or_default, .. } => {
            // `base.path or default` parses like an application argument, so
            // treat the `or` form as apply-strength: it then parenthesizes in
            // argument position (`f (x.y or d)`) instead of leaning on the
            // grammar's surprising greedy-`or` rule.
            if or_default.is_some() { APPLY } else { SELECT }
        }
        Expr::Apply { .. } => APPLY,
        Expr::Unary { op, .. } => match op {
            UnaryOp::Neg => NEG,
            UnaryOp::Not => NOT,
        },
        Expr::Binary { op, .. } => binary_strength(*op).strength,
        Expr::Lambda { .. } | Expr::Let { .. } | Expr::If { .. } => LOWEST,
    }
}

enum Assoc {
    Left,
    Right,
    /// Comparison and equality do not chain in Nix.
    None,
}

struct OpStrength {
    token: &'static str,
    strength: u8,
    assoc: Assoc,
}

const fn binary_strength(op: BinaryOp) -> OpStrength {
    let (token, strength, assoc) = match op {
        BinaryOp::Concat => ("++", CONCAT, Assoc::Right),
        BinaryOp::Mul => ("*", MUL, Assoc::Left),
        BinaryOp::Div => ("/", MUL, Assoc::Left),
        BinaryOp::Add => ("+", ADD, Assoc::Left),
        BinaryOp::Sub => ("-", ADD, Assoc::Left),
        BinaryOp::Update => ("//", UPDATE, Assoc::Right),
        BinaryOp::Lt => ("<", CMP, Assoc::None),
        BinaryOp::Le => ("<=", CMP, Assoc::None),
        BinaryOp::Gt => (">", CMP, Assoc::None),
        BinaryOp::Ge => (">=", CMP, Assoc::None),
        BinaryOp::Eq => ("==", EQUALITY, Assoc::None),
        BinaryOp::Ne => ("!=", EQUALITY, Assoc::None),
        BinaryOp::And => ("&&", AND, Assoc::Left),
        BinaryOp::Or => ("||", OR, Assoc::Left),
    };
    OpStrength {
        token,
        strength,
        assoc,
    }
}

fn newline(out: &mut String, indent: usize) {
    out.push('\n');
    for _ in 0..indent {
        out.push_str("  ");
    }
}

/// Writes `e`, parenthesizing when it binds more loosely than the context
/// tolerates.
fn expr(out: &mut String, e: &Expr, ctx: Ctx) {
    let parens = strength(e) < ctx.min;
    if parens {
        out.push('(');
    }
    let indent = ctx.indent;
    match e {
        Expr::Int(value) => {
            let _ = write!(out, "{value}");
        }
        Expr::Float(value) => float(out, *value),
        Expr::Str(parts) => string(out, parts, indent),
        Expr::Path(text) | Expr::Ident(text) => out.push_str(text),
        Expr::Select {
            base,
            path,
            or_default,
        } => {
            expr(out, base, ctx.with_min(ATOM));
            for key in path {
                out.push('.');
                attr(out, key, indent);
            }
            if let Some(default) = or_default {
                out.push_str(" or ");
                expr(out, default, ctx.with_min(SELECT));
            }
        }
        Expr::AttrSet(bindings) => attrset(out, bindings, indent),
        Expr::List(items) => list(out, items, indent),
        Expr::Lambda { param, body } => {
            lambda_param(out, param, indent);
            out.push_str(": ");
            expr(out, body, ctx.with_min(LOWEST));
        }
        Expr::Let { bindings, body } => {
            out.push_str("let");
            for binding in bindings {
                newline(out, indent + 1);
                out.push_str(&binding.name);
                out.push_str(" = ");
                expr(out, &binding.value, ctx.nested(LOWEST));
                out.push(';');
            }
            newline(out, indent);
            out.push_str("in");
            newline(out, indent);
            expr(out, body, ctx.with_min(LOWEST));
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            out.push_str("if ");
            expr(out, cond, ctx.with_min(OR));
            out.push_str(" then ");
            expr(out, then, ctx.with_min(OR));
            out.push_str(" else ");
            // `LOWEST` keeps `else if` chains and trailing lambdas flat.
            expr(out, otherwise, ctx.with_min(LOWEST));
        }
        Expr::Unary { op, operand } => {
            let (token, strength) = match op {
                UnaryOp::Not => ("!", NOT),
                UnaryOp::Neg => ("-", NEG),
            };
            out.push_str(token);
            expr(out, operand, ctx.with_min(strength + 1));
        }
        Expr::Binary { op, lhs, rhs } => {
            let op = binary_strength(*op);
            let (lhs_min, rhs_min) = match op.assoc {
                Assoc::Left => (op.strength, op.strength + 1),
                Assoc::Right => (op.strength + 1, op.strength),
                Assoc::None => (op.strength + 1, op.strength + 1),
            };
            expr(out, lhs, ctx.with_min(lhs_min));
            let _ = write!(out, " {} ", op.token);
            expr(out, rhs, ctx.with_min(rhs_min));
        }
        Expr::Apply { function, argument } => {
            expr(out, function, ctx.with_min(APPLY));
            out.push(' ');
            expr(out, argument, ctx.with_min(SELECT));
        }
    }
    if parens {
        out.push(')');
    }
}

/// Floats always carry a decimal point or exponent so Nix reads them back as
/// floats, not integers.
fn float(out: &mut String, value: f64) {
    let text = format!("{value}");
    let already_floaty = text.contains(['.', 'e', 'E']);
    out.push_str(&text);
    if !already_floaty {
        out.push_str(".0");
    }
}

fn string(out: &mut String, parts: &[StrPart], indent: usize) {
    out.push('"');
    for part in parts {
        match part {
            StrPart::Lit(text) => escape_into(out, text),
            StrPart::Interp(inner) => {
                out.push_str("${");
                expr(
                    out,
                    inner,
                    Ctx {
                        min: LOWEST,
                        indent,
                    },
                );
                out.push('}');
            }
        }
    }
    out.push('"');
}

/// Escapes literal text per Nix `"` string rules.
fn escape_into(out: &mut String, text: &str) {
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            other => out.push(other),
        }
    }
}

fn attrset(out: &mut String, bindings: &[Binding], indent: usize) {
    if bindings.is_empty() {
        out.push_str("{}");
        return;
    }
    out.push('{');
    for binding in bindings {
        newline(out, indent + 1);
        attr(out, &binding.key, indent + 1);
        out.push_str(" = ");
        expr(
            out,
            &binding.value,
            Ctx {
                min: LOWEST,
                indent: indent + 1,
            },
        );
        out.push(';');
    }
    newline(out, indent);
    out.push('}');
}

fn list(out: &mut String, items: &[Expr], indent: usize) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push_str("[ ");
    for item in items {
        expr(
            out,
            item,
            Ctx {
                min: SELECT,
                indent,
            },
        );
        out.push(' ');
    }
    out.push(']');
}

fn attr(out: &mut String, key: &Attr, indent: usize) {
    match key {
        Attr::Name(name) => {
            if is_bare_ident(name) {
                out.push_str(name);
            } else {
                out.push('"');
                escape_into(out, name);
                out.push('"');
            }
        }
        Attr::Dynamic(inner) => {
            out.push_str("${");
            expr(
                out,
                inner,
                Ctx {
                    min: LOWEST,
                    indent,
                },
            );
            out.push('}');
        }
    }
}

fn lambda_param(out: &mut String, param: &Param, indent: usize) {
    match param {
        Param::Ident(name) => out.push_str(name),
        Param::Pattern {
            fields,
            ellipsis,
            bind,
        } => {
            out.push_str("{ ");
            let mut first = true;
            for field in fields {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                out.push_str(&field.name);
                if let Some(default) = &field.default {
                    out.push_str(" ? ");
                    expr(
                        out,
                        default,
                        Ctx {
                            min: LOWEST,
                            indent,
                        },
                    );
                }
            }
            if *ellipsis {
                if !first {
                    out.push_str(", ");
                }
                out.push_str("...");
                first = false;
            }
            out.push_str(if first { "}" } else { " }" });
            if let Some(bind) = bind {
                let _ = write!(out, " @ {bind}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nix::{Binding, LetBinding, PatternField};

    /// Renders `body` and strips the module wrapper, so each case asserts
    /// only the expression under test.
    fn rendered(body: Expr) -> String {
        let out = module(&Module { body });
        out.strip_prefix("{ __dir, __importIx, __ixTy }:\n")
            .expect("every module renders under the wrapper")
            .to_owned()
    }

    fn ident(name: &str) -> Expr {
        Expr::Ident(name.into())
    }

    fn apply(function: Expr, argument: Expr) -> Expr {
        Expr::Apply {
            function: Box::new(function),
            argument: Box::new(argument),
        }
    }

    fn binary(op: BinaryOp, lhs: Expr, rhs: Expr) -> Expr {
        Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        }
    }

    #[test]
    fn application_argument_gets_parens_but_chain_does_not() {
        let nested = apply(ident("f"), apply(ident("g"), ident("x")));
        assert_eq!(rendered(nested), "f (g x)\n");

        let curried = apply(apply(ident("f"), ident("a")), ident("b"));
        assert_eq!(rendered(curried), "f a b\n");
    }

    #[test]
    fn select_with_or_parenthesizes_as_argument() {
        let or_select = Expr::Select {
            base: Box::new(ident("x")),
            path: vec![Attr::Name("y".into())],
            or_default: Some(Box::new(ident("d"))),
        };
        assert_eq!(rendered(apply(ident("f"), or_select)), "f (x.y or d)\n");
    }

    #[test]
    fn select_base_stronger_than_application_needs_parens() {
        let select = Expr::Select {
            base: Box::new(apply(ident("f"), ident("a"))),
            path: vec![Attr::Name("b".into())],
            or_default: None,
        };
        assert_eq!(rendered(select), "(f a).b\n");
    }

    #[test]
    fn left_folded_update_keeps_left_grouping_explicit() {
        // `//` is right-associative in Nix, so the mapper's left fold must
        // parenthesize: `(a // b) // c`, not `a // b // c`.
        let folded = binary(
            BinaryOp::Update,
            binary(BinaryOp::Update, ident("a"), ident("b")),
            ident("c"),
        );
        assert_eq!(rendered(folded), "(a // b) // c\n");
    }

    #[test]
    fn subtraction_grouping_survives_rendering() {
        let rhs_grouped = binary(
            BinaryOp::Sub,
            ident("a"),
            binary(BinaryOp::Sub, ident("b"), ident("c")),
        );
        assert_eq!(rendered(rhs_grouped), "a - (b - c)\n");

        let lhs_grouped = binary(
            BinaryOp::Sub,
            binary(BinaryOp::Sub, ident("a"), ident("b")),
            ident("c"),
        );
        assert_eq!(rendered(lhs_grouped), "a - b - c\n");
    }

    #[test]
    fn lambda_in_operand_position_gets_parens() {
        let negated = Expr::Unary {
            op: UnaryOp::Not,
            operand: Box::new(Expr::Lambda {
                param: Param::Ident("x".into()),
                body: Box::new(ident("x")),
            }),
        };
        assert_eq!(rendered(negated), "!(x: x)\n");
    }

    #[test]
    fn keyword_and_exotic_attr_names_render_quoted() {
        let attrset = Expr::AttrSet(vec![
            Binding {
                key: Attr::Name("then".into()),
                value: Expr::Int(1),
            },
            Binding {
                key: Attr::Name("a b".into()),
                value: Expr::Int(2),
            },
        ]);
        assert_eq!(rendered(attrset), "{\n  \"then\" = 1;\n  \"a b\" = 2;\n}\n");
    }

    #[test]
    fn strings_escape_nix_specials() {
        let text = Expr::Str(vec![StrPart::Lit("a\"b\\c${d}\n".into())]);
        assert_eq!(rendered(text), "\"a\\\"b\\\\c\\${d}\\n\"\n");
    }

    #[test]
    fn floats_always_carry_a_decimal_marker() {
        assert_eq!(rendered(Expr::Float(2.0)), "2.0\n");
        assert_eq!(rendered(Expr::Float(1.5)), "1.5\n");
        // Rust never uses exponent notation for `f64` Display; the invariant
        // is only that a decimal marker survives.
        assert!(rendered(Expr::Float(1e300)).ends_with(".0\n"));
    }

    #[test]
    fn empty_and_defaulted_patterns_render() {
        let empty = Expr::Lambda {
            param: Param::Pattern {
                fields: vec![],
                ellipsis: false,
                bind: None,
            },
            body: Box::new(Expr::Int(1)),
        };
        assert_eq!(rendered(empty), "{ }: 1\n");

        let full = Expr::Lambda {
            param: Param::Pattern {
                fields: vec![
                    PatternField {
                        name: "a".into(),
                        default: None,
                    },
                    PatternField {
                        name: "b".into(),
                        default: Some(Expr::Int(1)),
                    },
                ],
                ellipsis: true,
                bind: Some("rest".into()),
            },
            body: Box::new(ident("a")),
        };
        assert_eq!(rendered(full), "{ a, b ? 1, ... } @ rest: a\n");
    }

    #[test]
    fn let_bindings_indent_deterministically() {
        let let_in = Expr::Let {
            bindings: vec![LetBinding {
                name: "a".into(),
                value: Expr::Int(1),
            }],
            body: Box::new(ident("a")),
        };
        assert_eq!(rendered(let_in), "let\n  a = 1;\nin\na\n");
    }
}
