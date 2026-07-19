//! Parser for the plumb shell: a strict, spanned bash subset.
//!
//! The grammar covers simple commands, quoting, `$VAR`/`${VAR}`/`$?`,
//! command substitution `$(...)`, pipes, `&&`/`||`/`;`, redirections,
//! globs, `NAME=value` prefixes, and a trailing `&`. Every construct the
//! parser accepts means the same thing in bash; everything else (keywords,
//! subshells, backticks, here-docs, fancy parameter expansion, brace
//! expansion) is a [`ParseError`] naming the unsupported construct.

mod ast;
mod parse;

pub use ast::{
    Assign, AndOr, AndOrTail, Command, Connector, Item, Part, Pipeline, Program, Redirect,
    RedirOp, Span, Word,
};
pub use parse::{parse, ParseError};
