//! Content-addressed semantic code search, deduplicated across git worktrees.
//!
//! The design in one paragraph: files are identified by the hash of their
//! bytes, not their path, so byte-identical files across many worktrees,
//! branches, or repos share a single stored embedding. Sync uploads only
//! content the store is missing and never deletes (deletion across shared
//! entries needs reference counting, which belongs in a separate garbage
//! collection pass). A local [`Manifest`] records this checkout's `path ->
//! hash` view; search over-fetches from the shared store and keeps only hits
//! whose hash is in the manifest, so results reflect the current tree. There
//! is no daemon: each run rebuilds the manifest cheaply (mtime skips
//! re-hashing) and uploads what is new.
//!
//! The [`Store`] trait abstracts the backend so sync and search are tested
//! against [`MemoryStore`]; [`MixedbreadStore`] is the production adapter over
//! the standalone [`mixedbread`] client crate.

// `ContentHash` and `SyncReport` deliberately echo their modules (`content`,
// `sync`). The module split is internal and everything is re-exported at the
// crate root, so the repetition never reaches callers.
#![allow(clippy::module_name_repetitions)]

mod adapter;
mod backend;
mod config;
mod content;
mod context;
mod db;
mod error;
mod manifest;
mod pipeline;
mod query_filter;
mod repo;
mod search;
mod sync;

pub use adapter::MixedbreadStore;
pub use backend::{
    Answer, AskOptions, GrepOptions, GrepTargets, MemoryStore, Provenance, SearchHit,
    SearchOptions, Store, StoreStatus, StoredRecord,
};
pub use config::{Config, DEFAULT_STORE, WEB_STORE};
pub use content::ContentHash;
pub use context::{ContextView, context};
pub use db::{Db, db_path};
pub use error::{Error, Result};
pub use manifest::{FileEntry, Manifest};
pub use pipeline::{Query, index_and_answer, index_and_grep, index_and_semantic};
pub use query_filter::{FilterSpec, InvalidTimeSpec, build_filter, parse_time_spec};
pub use repo::repo_slug;
pub use search::{
    AnswerView, CodeScope, COMPACT_SNIPPET_CHARS, DisplayHit, RenderMode, SourceStat, ask, grep,
    hits_to_json, ranked, recent, semantic, stats,
};
pub use sync::{SyncReport, sync, wait_until_indexed};

// Re-export the shared metadata and filter types so binaries depend only on
// search-core.
pub use mixedbread::{
    Agentic, AgenticConfig, Condition, DEFAULT_RERANK_MODEL, EnhancedQuery, EventType,
    FACETS_MAX_FILES, FacetLimits, Facets, FileIds, FileStatus, Filter, FilterMode, Group,
    HistogramBucket, Operator, Rerank, SortBy, SortDirection,
};
pub use source_meta::{Document, KNOWN_SOURCE_TAGS, RepoSlug, Source, SourceAdapter};
