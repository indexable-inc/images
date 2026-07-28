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
    AndOr, AndOrTail, Assign, Command, Connector, Item, Part, PathSeg, Pipeline, Program,
    RedirOp, Redirect, Span, Word,
};
pub use parse::{parse, ParseError};
