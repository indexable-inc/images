mod format;
mod parse;
mod types;

pub use format::{RevisionError, conflict, extract_oid_from_marker, read_revision, write_result};
pub use parse::conflicts;
pub use types::{DisplaySettings, DriverResult, ParsedConflict, ParsedFile};

#[cfg(test)]
mod tests;
