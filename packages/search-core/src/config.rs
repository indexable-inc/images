//! Runtime configuration with conservative, production-shaped defaults. The
//! API base URL is owned by the [`mixedbread`] crate, not duplicated here.

/// Default store name: the shared corpus the `indexer` populates (code plus
/// agent/shell history across the fleet). One store holds everything; queries
/// scope it server-side with a metadata filter rather than using separate stores.
pub const DEFAULT_STORE: &str = "index";

/// Identifier of Mixedbread's hosted web-search store, mixed in when the
/// caller opts into web results.
pub const WEB_STORE: &str = "mixedbread/web";

/// Tunable limits for indexing and sync.
#[derive(Debug, Clone)]
pub struct Config {
    /// Files larger than this are skipped during indexing. Defaults to 1 MiB.
    pub max_file_bytes: u64,
    /// Upper bound on how many new files one sync may upload before it refuses
    /// to run, guarding against an accidental index of a huge tree.
    pub max_files: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_file_bytes: 1024 * 1024,
            max_files: 10_000,
        }
    }
}
