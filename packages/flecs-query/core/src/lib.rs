//! Pure-Rust parser for the [Flecs Query Language], the string format flecs
//! uses for queries (`Position, [in] Velocity, (ChildOf, $parent)`).
//!
//! Flecs itself has no standalone grammar artifact: the language is defined
//! by a hand-written C parser (`addons/query_dsl/parser.c`) that is
//! inseparable from a live `ecs_world_t`. This crate reimplements the same
//! grammar as a standalone, world-independent parser: [`parse`] turns an
//! expression into a typed [`Query`] AST, and the AST's [`Display`] renders
//! the canonical form back (`parse(q.to_string()) == q`).
//!
//! [Flecs Query Language]: https://github.com/SanderMertens/flecs/blob/master/docs/FlecsQueryLanguage.md
//!
//! # Grammar
//!
//! The grammar, reverse-engineered from the upstream parser and test suite
//! (flecs has never written one down):
//!
//! ```ebnf
//! query     = "0" | term { ( "," | "||" ) term } ;
//! term      = [ access ] ( scope | pair | id-term ) ;
//! access    = "[" ( "default" | "in" | "out" | "inout" | "none" | "filter" ) "]" ;
//! scope     = [ "!" | "?" ] "{" query "}" ;
//! pair      = [ "!" | "?" | keyword "|" ] "(" ref [ "|" trav ] "," args ")" ;
//! id-term   = ( "!" | "?" ) unary-body
//!           | keyword "|" keyword-body
//!           | eq-term
//!           | component ;
//! keyword   = "and" | "or" | "not" | "auto_override" | "toggle" ;
//! component = ref [ "|" trav ] [ "(" [ args ] ")" ] ;
//! eq-term   = ref ( "==" | "!=" | "~=" ) ( ref | string ) ;
//! args      = arg { ( "," | "||" ) arg } ;
//! arg       = trav | "@" ref | ref [ "|" trav ] ;
//! trav      = flag { "|" flag } [ identifier ] ;
//! flag      = "self" | "up" | "cascade" | "desc" ;
//! ref       = identifier | "*" | number ;
//! ```
//!
//! Identifiers cover names (`Position`), lookup paths (`flecs.meta.Member`),
//! member access (`Movement.direction`), template types (`Position<int>`),
//! variables (`$food`, `$this`, bare `$`), entity ids (`#511`), the `_` any
//! wildcard, and `\ `-escaped characters. `//` and `/* */` comments and all
//! whitespace (including newlines) are insignificant, as in upstream's query
//! parsing mode.
//!
//! # What this crate checks, and what it cannot
//!
//! [`parse`] answers "is this well-formed Flecs Query Language" and produces
//! the structure of every term. Whether `Position` names a real component is
//! a property of a specific world; flecs resolves identifiers with
//! `ecs_lookup` at query-creation time, and a parser without a world cannot
//! (and should not) guess. Consumers that need resolution can walk the AST
//! and look names up against whatever backend they have.
//!
//! # Deliberate deviations from upstream
//!
//! Upstream *silently ignores* an unknown access modifier (`[foo]`) and
//! silently drops an unknown word after `|` where a traversal flag belongs;
//! this parser rejects both with an error. Some upstream errors that only
//! surface in the world-dependent validator (unbalanced `{`/`}` scopes)
//! are reported here at parse time instead.
//!
//! # Example
//!
//! ```
//! use flecs_query_core::{parse, Oper, TermBody};
//!
//! let query = parse("Position, [in] Velocity, !Dead, (ChildOf, $parent)").unwrap();
//! assert_eq!(query.terms.len(), 4);
//! assert_eq!(query.terms[2].oper, Oper::Not);
//!
//! // Round-trip through the canonical form.
//! assert_eq!(parse(&query.to_string()).unwrap(), query);
//! ```

mod ast;
mod error;
mod fmt;
mod parser;
mod token;

pub use ast::{
    Access, EqOp, EqOperand, EqTerm, ExtraOper, IdFlag, IdTerm, Oper, Query, Ref, RefExpr, Src,
    Term, TermBody, Traversal,
};
pub use error::{ParseError, Span};
pub use parser::parse;

impl std::str::FromStr for Query {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse(s)
    }
}
