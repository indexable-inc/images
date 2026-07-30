//! `.memories`: one markdown file per lesson, per repo, ranked on search.
//!
//! A memory is YAML frontmatter (`tldr`, `genre`, `topic`, `handle`, `prior`,
//! `based_on`, `validated`) plus a markdown body. Discovery is a directory
//! listing, so there is no index to rebuild and no manifest to drift; ranking is
//! BM25 with per-field boosts times the confidence, age and reinforcement
//! factors in [`rank`]; and every file that does not parse becomes a
//! diagnostic rather than a silent skip.
//!
//! Start at [`discover::load`] to read a corpus, [`search::search`] to rank it,
//! [`lint::lint`] to check it, and [`write`] for the only code that writes the
//! on-disk format.

pub mod discover;
pub mod error;
pub mod lint;
pub mod model;
pub mod rank;
pub mod report;
pub mod search;
pub mod secret;
pub mod stale;
pub mod write;

#[cfg(test)]
mod fixture;

pub use error::{Error, Result};
