//! The typed, world-independent AST for a parsed query expression.
//!
//! The shapes mirror flecs's `ecs_term_t` model (first/source/second
//! references, one operator and one access modifier per term) but stay at the
//! syntax level: identifiers are kept as names instead of being resolved to
//! entity ids, which is what makes the AST usable without a live world.

/// A parsed query: the comma-separated list of terms.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Query {
    /// The conditions an entity must satisfy, in source order.
    pub terms: Vec<Term>,
}

/// One condition in a query.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Term {
    /// The `[in]`/`[out]`-style access modifier, when written.
    pub access: Option<Access>,
    /// How the term combines with the query (`!`, `?`, `||`, `and|`, ...).
    pub oper: Oper,
    /// What the term matches.
    pub body: TermBody,
}

/// How a term combines with the rest of the query.
///
/// `Or` means "or with the next term": flecs marks the left-hand term of
/// `A || B` and leaves the right-hand term as `And`, and this AST keeps that
/// convention so terms stay a flat list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Oper {
    /// Plain conjunction (the default).
    And,
    /// `A || B`: at least one of the chained terms must match.
    Or,
    /// `!A`: the entity must not match.
    Not,
    /// `?A`: matched if present, without constraining the result set.
    Optional,
    /// `and|Type`: match all components the `Type` entity has.
    AndFrom,
    /// `or|Type`: match at least one component the `Type` entity has.
    OrFrom,
    /// `not|Type`: match none of the components the `Type` entity has.
    NotFrom,
}

/// The `[...]` access modifier on a term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Access {
    /// `[default]`: infer from ownership (`inout` for owned, `in` for shared).
    Default,
    /// `[in]`: read-only.
    In,
    /// `[out]`: write-only.
    Out,
    /// `[inout]`: read-write.
    InOut,
    /// `[none]`: matched but never accessed.
    None,
    /// `[filter]`: matched without producing observer events.
    Filter,
}

/// What a term matches.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TermBody {
    /// A component, tag, or pair term.
    Id(IdTerm),
    /// An equality predicate: `$this == Foo`, `$x != $y`, `$this ~= "Uss"`.
    Eq(EqTerm),
    /// A `{ ... }` scope; the operator applies to the group as a whole.
    Scope(Vec<Term>),
}

/// A component/pair term: `first(src, second)` in its most explicit form.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct IdTerm {
    /// An id flag applied with the keyword-pipe syntax (`auto_override|`,
    /// `toggle|`).
    pub flag: Option<IdFlag>,
    /// The component or relationship being matched.
    pub first: Ref,
    /// The entity the term is matched on.
    pub src: Src,
    /// The pair target, when the term matches a pair.
    pub second: Option<Ref>,
    /// Targets beyond the second: `Rel(src, a, b)` (flecs unpacks these into
    /// chained terms; the AST keeps the surface form).
    pub extra: Vec<Ref>,
    /// How [`Self::extra`] targets chain: `Rel(src, a, b)` vs `Rel(src, a || b)`.
    pub extra_oper: ExtraOper,
}

/// How extra pair targets combine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ExtraOper {
    /// `Rel(x, y, z)`: equivalent to `Rel(x, y), Rel(y, z)`.
    #[default]
    And,
    /// `Rel(x, y || z)`: equivalent to `Rel(x, y) || Rel(x, z)`.
    Or,
}

/// An id flag written with the keyword-pipe syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IdFlag {
    /// `auto_override|Comp`: the component is auto-overridden on instantiation.
    AutoOverride,
    /// `toggle|Comp`: the component can be enabled/disabled per entity.
    Toggle,
}

/// The source of a term: the entity the condition is evaluated on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Src {
    /// No parentheses written; defaults to `$this`.
    #[default]
    Implicit,
    /// `Comp()`: explicitly matched on no entity.
    Empty,
    /// `Comp(Game)`, `Comp($var)`, `Comp(self|up ChildOf)`, ...
    Explicit(Ref),
}

/// A reference to an entity-like operand, with optional traversal.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Ref {
    /// The operand itself.
    pub expr: RefExpr,
    /// Traversal flags attached with `|` (`src|self`, `self|up IsA`).
    pub traversal: Option<Traversal>,
}

impl Ref {
    /// A reference with no traversal flags.
    #[must_use]
    pub const fn plain(expr: RefExpr) -> Self {
        Self {
            expr,
            traversal: None,
        }
    }
}

/// An entity-like operand in a term.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RefExpr {
    /// A name or dot-separated lookup path (`Position`, `flecs.meta.Member`,
    /// `Movement.direction`, `Position<int>`). Escaped dots are kept as `\.`.
    Name(String),
    /// The builtin `$this` variable.
    This,
    /// A query variable (`$food`). The empty name is the bare `$` source,
    /// which matches the component on itself (singleton terms).
    Var(String),
    /// The `*` wildcard: match all instances.
    Wildcard,
    /// The `_` wildcard: match at most one instance.
    Any,
    /// A raw entity id (`#511`, or a bare number in an operand position).
    /// `#0` (id zero) is the explicit empty source.
    Entity(u64),
    /// A `@`-prefixed value operand for value pairs (`@*`, `@Red`, `@7`).
    Value(Box<Self>),
    /// No operand written, only traversal flags: the source in
    /// `Position(self|up ChildOf)`. The entity is implied by position
    /// (`$this` for sources).
    Implied,
}

/// Traversal flags on a reference, plus the optional relationship to traverse.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the four flags mirror upstream's EcsSelf/EcsUp/EcsCascade/EcsDesc bitflags, which combine freely"
)]
pub struct Traversal {
    /// `self`: match on the entity itself.
    pub self_: bool,
    /// `up`: traverse the relationship upwards.
    pub up: bool,
    /// `cascade`: like `up`, breadth-first ordered results.
    pub cascade: bool,
    /// `desc`: reverse `cascade` order.
    pub desc: bool,
    /// The relationship to traverse (`up ChildOf`); flecs defaults to
    /// `ChildOf` when omitted.
    pub rel: Option<String>,
}

impl Traversal {
    /// Whether any flag or relationship was written.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        !self.self_ && !self.up && !self.cascade && !self.desc && self.rel.is_none()
    }
}

/// An equality predicate term.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EqTerm {
    /// The left operand (becomes the term source in flecs).
    pub left: RefExpr,
    /// `==`/`!=` or `~=`. Negation lives on the term operator: `!=` parses as
    /// [`Oper::Not`] + [`EqOp::Eq`], and `~= "!str"` as [`Oper::Not`] +
    /// [`EqOp::Match`], exactly as flecs encodes them.
    pub op: EqOp,
    /// The right operand: an entity-like expression or a name string.
    pub right: EqOperand,
}

/// The comparison applied by an equality term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EqOp {
    /// `==` (or `!=` with [`Oper::Not`]).
    Eq,
    /// `~=`: substring match on the entity name.
    Match,
}

/// The right operand of an equality term.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum EqOperand {
    /// An entity-like operand (`UssEnterprise`, `$other`, `*`).
    Ref(RefExpr),
    /// A quoted string, compared against the entity name.
    Name(String),
}
