//! Canonical rendering of the AST back to query-DSL text.
//!
//! `parse(query.to_string())` reproduces the same AST. The output is
//! canonical rather than byte-identical to the input: whitespace is
//! normalized, comments are dropped, raw entity ids render as `#id`, and an
//! implicit-source pair renders in `(Rel, Tgt)` form.

use std::fmt::{Display, Formatter, Result, Write as _};

use crate::ast::{
    Access, EqOp, EqOperand, EqTerm, ExtraOper, IdFlag, IdTerm, Oper, Query, Ref, RefExpr, Src,
    Term, TermBody, Traversal,
};

impl Display for Query {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        if self.terms.is_empty() {
            // The canonical empty query, accepted back by `parse`.
            return f.write_str("0");
        }
        write_terms(f, &self.terms)
    }
}

fn write_terms(f: &mut Formatter<'_>, terms: &[Term]) -> Result {
    for (index, term) in terms.iter().enumerate() {
        if index > 0 {
            let previous = &terms[index - 1];
            f.write_str(if previous.oper == Oper::Or { " || " } else { ", " })?;
        }
        write!(f, "{term}")?;
    }
    Ok(())
}

impl Display for Term {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        if let Some(access) = self.access {
            write!(f, "{access} ")?;
        }

        // `!=` and `~= "!..."` carry the negation themselves; every other
        // body takes a prefix for its operator.
        let negated_eq = self.oper == Oper::Not && matches!(self.body, TermBody::Eq(_));
        match self.oper {
            Oper::And | Oper::Or => {}
            Oper::Not if negated_eq => {}
            Oper::Not => f.write_str("!")?,
            Oper::Optional => f.write_str("?")?,
            Oper::AndFrom => f.write_str("and|")?,
            Oper::OrFrom => f.write_str("or|")?,
            Oper::NotFrom => f.write_str("not|")?,
        }

        match &self.body {
            TermBody::Id(id) => write!(f, "{id}"),
            TermBody::Eq(eq) => write_eq(f, eq, negated_eq),
            TermBody::Scope(terms) => {
                f.write_str("{")?;
                write_terms(f, terms)?;
                f.write_str("}")
            }
        }
    }
}

impl Display for Access {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_str(match self {
            Self::Default => "[default]",
            Self::In => "[in]",
            Self::Out => "[out]",
            Self::InOut => "[inout]",
            Self::None => "[none]",
            Self::Filter => "[filter]",
        })
    }
}

impl Display for IdTerm {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self.flag {
            Some(IdFlag::AutoOverride) => f.write_str("auto_override|")?,
            Some(IdFlag::Toggle) => f.write_str("toggle|")?,
            None => {}
        }

        let separator = match self.extra_oper {
            ExtraOper::And => ", ",
            ExtraOper::Or => " || ",
        };

        match (&self.src, &self.second) {
            (Src::Implicit, None) => write!(f, "{first}", first = self.first),
            // Canonical pair form for an implicit source.
            (Src::Implicit, Some(second)) => {
                write!(f, "({first}, {second}", first = self.first)?;
                for extra in &self.extra {
                    write!(f, "{separator}{extra}")?;
                }
                f.write_str(")")
            }
            (Src::Empty, _) => write!(f, "{first}()", first = self.first),
            (Src::Explicit(src), second) => {
                write!(f, "{first}({src}", first = self.first)?;
                if let Some(second) = second {
                    write!(f, ", {second}")?;
                    for extra in &self.extra {
                        write!(f, "{separator}{extra}")?;
                    }
                }
                f.write_str(")")
            }
        }
    }
}

fn write_eq(f: &mut Formatter<'_>, eq: &EqTerm, negated: bool) -> Result {
    write!(f, "{left} ", left = eq.left)?;
    match (eq.op, negated, &eq.right) {
        (EqOp::Eq, false, _) => f.write_str("== ")?,
        (EqOp::Eq, true, _) => f.write_str("!= ")?,
        // The match negation can only be written inside a string operand.
        (EqOp::Match, true, EqOperand::Name(name)) => {
            return write!(f, "~= \"!{escaped}\"", escaped = EscapedString(name));
        }
        (EqOp::Match, _, _) => f.write_str("~= ")?,
    }
    match &eq.right {
        EqOperand::Ref(expr) => write!(f, "{expr}"),
        EqOperand::Name(name) => write!(f, "\"{escaped}\"", escaped = EscapedString(name)),
    }
}

impl Display for Ref {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match (&self.expr, &self.traversal) {
            (RefExpr::Implied, Some(traversal)) => write!(f, "{traversal}"),
            (RefExpr::Implied, None) => Ok(()),
            (expr, Some(traversal)) => write!(f, "{expr}|{traversal}"),
            (expr, None) => write!(f, "{expr}"),
        }
    }
}

impl Display for Traversal {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let mut first = true;
        for (set, flag) in [
            (self.self_, "self"),
            (self.up, "up"),
            (self.cascade, "cascade"),
            (self.desc, "desc"),
        ] {
            if set {
                if !first {
                    f.write_str("|")?;
                }
                f.write_str(flag)?;
                first = false;
            }
        }
        if let Some(rel) = &self.rel {
            if !first {
                f.write_str(" ")?;
            }
            f.write_str(rel)?;
        }
        Ok(())
    }
}

impl Display for RefExpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            Self::Name(name) => write!(f, "{escaped}", escaped = EscapedName(name)),
            Self::This => f.write_str("$this"),
            Self::Var(name) => write!(f, "${name}"),
            Self::Wildcard => f.write_str("*"),
            Self::Any => f.write_str("_"),
            Self::Entity(id) => write!(f, "#{id}"),
            Self::Value(inner) => write!(f, "@{inner}"),
            Self::Implied => Ok(()),
        }
    }
}

/// A name with non-identifier characters re-escaped, inverting the
/// tokenizer's unescaping (stored `\.` sequences pass through verbatim).
struct EscapedName<'a>(&'a str);

impl Display for EscapedName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        let mut chars = self.0.chars();
        let mut template_depth = 0_usize;
        while let Some(c) = chars.next() {
            match c {
                // Template arguments (`Map<string, vector<int>>`) tokenize
                // verbatim, so they render verbatim too.
                '<' => {
                    template_depth += 1;
                    f.write_char(c)?;
                }
                '>' => {
                    template_depth = template_depth.saturating_sub(1);
                    f.write_char(c)?;
                }
                _ if template_depth > 0 => f.write_char(c)?,
                '\\' => {
                    f.write_char('\\')?;
                    if let Some(next) = chars.next() {
                        f.write_char(next)?;
                    }
                }
                _ if c.is_ascii_alphanumeric() || "_$#.*".contains(c) => f.write_char(c)?,
                _ => {
                    f.write_char('\\')?;
                    f.write_char(c)?;
                }
            }
        }
        Ok(())
    }
}

/// A string operand with `"` and `\` escaped.
struct EscapedString<'a>(&'a str);

impl Display for EscapedString<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        for c in self.0.chars() {
            if matches!(c, '"' | '\\') {
                f.write_char('\\')?;
            }
            f.write_char(c)?;
        }
        Ok(())
    }
}
