//! The typed Nix expression tree that [`crate::map`] produces and
//! [`crate::render`] prints.
//!
//! This is the only vocabulary the two passes share: nothing outside the
//! renderer ever assembles Nix syntax from strings.

/// A rendered `.ix` module. Every module renders under the
/// `{ __dir, __importIx }:` wrapper so importers have exactly one calling
/// convention, whether or not the source used `import()`.
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub body: Expr,
}

/// A Nix expression. Deliberately smaller than Nix itself: only the forms the
/// `JavaScript` skin can reach exist (no `rec`, no `with`, no `assert`, no
/// `inherit`, no `?` has-attr).
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(i64),
    Float(f64),
    /// A `"..."` string built from literal and `${...}` interpolation parts.
    Str(Vec<StrPart>),
    /// A literal path such as `./x.nix`, emitted verbatim.
    Path(String),
    Ident(String),
    /// `base.a.b` with an optional `or` default: `base.a.b or default`.
    Select {
        base: Box<Self>,
        path: Vec<Attr>,
        or_default: Option<Box<Self>>,
    },
    /// `{ k = v; ... }`. Never recursive: `.ix` consts become `let`, not `rec`.
    AttrSet(Vec<Binding>),
    List(Vec<Self>),
    Lambda {
        param: Param,
        body: Box<Self>,
    },
    Let {
        bindings: Vec<LetBinding>,
        body: Box<Self>,
    },
    If {
        cond: Box<Self>,
        then: Box<Self>,
        otherwise: Box<Self>,
    },
    Unary {
        op: UnaryOp,
        operand: Box<Self>,
    },
    Binary {
        op: BinaryOp,
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    /// Function application, one argument at a time: `f a`.
    Apply {
        function: Box<Self>,
        argument: Box<Self>,
    },
}

/// One piece of a `"..."` string.
#[derive(Debug, Clone, PartialEq)]
pub enum StrPart {
    Lit(String),
    Interp(Expr),
}

/// One attrset-key or attrpath component.
#[derive(Debug, Clone, PartialEq)]
pub enum Attr {
    /// A static name; the renderer decides between bare-identifier and
    /// quoted-string spelling.
    Name(String),
    /// A dynamic `${expr}` key.
    Dynamic(Expr),
}

/// One `key = value;` binding inside an attrset.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub key: Attr,
    pub value: Expr,
}

/// One `name = value;` binding inside a `let`.
#[derive(Debug, Clone, PartialEq)]
pub struct LetBinding {
    pub name: String,
    pub value: Expr,
}

/// A lambda parameter: a plain identifier or an attrset pattern.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Ident(String),
    /// `{ a, b ? default, ... } @ bind`.
    Pattern {
        fields: Vec<PatternField>,
        /// True when the pattern accepts extra attributes (`...`).
        ellipsis: bool,
        /// The `@ name` binding for the whole attrset, when present.
        bind: Option<String>,
    },
}

/// One formal inside an attrset pattern, with its optional `?` default.
#[derive(Debug, Clone, PartialEq)]
pub struct PatternField {
    pub name: String,
    pub default: Option<Expr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `!`
    Not,
    /// Arithmetic negation `-`.
    Neg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    /// Attrset update `//`, the target of object spreads.
    Update,
    /// List concatenation `++`, the target of array spreads.
    Concat,
}

/// Nix keywords (plus `or`, which is contextual but unusable as a plain
/// identifier): these can never be emitted as bare identifiers.
pub const KEYWORDS: [&str; 10] = [
    "assert", "else", "if", "in", "inherit", "let", "or", "rec", "then", "with",
];

/// Whether `name` is spellable as a bare Nix identifier (`[A-Za-z_][A-Za-z0-9_'-]*`
/// and not a keyword).
#[must_use]
pub fn is_bare_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let head_ok = first.is_ascii_alphabetic() || first == '_';
    let tail_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '\'' | '-'));
    head_ok && tail_ok && !KEYWORDS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::is_bare_ident;

    #[test]
    fn bare_ident_accepts_nix_identifier_grammar() {
        for name in ["a", "_a", "a-b", "a'b", "A9_"] {
            assert!(is_bare_ident(name), "{name} should be bare");
        }
    }

    #[test]
    fn bare_ident_rejects_keywords_and_invalid_shapes() {
        for name in ["", "1a", "$x", "a b", "then", "or", "let", "-a"] {
            assert!(!is_bare_ident(name), "{name} should not be bare");
        }
    }
}
