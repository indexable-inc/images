//! Typed failures for the lake. Every decode failure names the column and row
//! rather than defaulting, mirroring `source-parquet`'s malformed-log posture.

use snafu::Snafu;

/// Failures from the Iceberg corpus lake.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
#[non_exhaustive]
pub enum Error {
    /// The lake table schema could not be built.
    #[snafu(display("failed to build the lake table schema"))]
    Schema {
        /// Underlying iceberg error (boxed: it is large, and errors are cold).
        #[snafu(source(from(iceberg::Error, Box::new)))]
        source: Box<iceberg::Error>,
    },
    /// The REST catalog could not be connected.
    #[snafu(display("failed to connect the Iceberg catalog at {uri}"))]
    Connect {
        /// Catalog URI dialed.
        uri: String,
        /// Underlying iceberg error (boxed: it is large, and errors are cold).
        #[snafu(source(from(iceberg::Error, Box::new)))]
        source: Box<iceberg::Error>,
    },
    /// The corpus namespace or table could not be created or loaded.
    #[snafu(display("failed to ensure the lake table {table}"))]
    EnsureTable {
        /// Table identifier.
        table: String,
        /// Underlying iceberg error (boxed: it is large, and errors are cold).
        #[snafu(source(from(iceberg::Error, Box::new)))]
        source: Box<iceberg::Error>,
    },
    /// The lake table could not be loaded from the catalog.
    #[snafu(display("failed to load the lake table {table}"))]
    LoadTable {
        /// Table identifier.
        table: String,
        /// Underlying iceberg error (boxed: it is large, and errors are cold).
        #[snafu(source(from(iceberg::Error, Box::new)))]
        source: Box<iceberg::Error>,
    },
    /// A table scan failed.
    #[snafu(display("failed to scan the lake table ({stage})"))]
    Scan {
        /// Which scan stage failed.
        stage: &'static str,
        /// Underlying iceberg error (boxed: it is large, and errors are cold).
        #[snafu(source(from(iceberg::Error, Box::new)))]
        source: Box<iceberg::Error>,
    },
    /// The append record batch could not be assembled.
    #[snafu(display("failed to build the append record batch"))]
    Batch {
        /// Underlying arrow error.
        source: arrow_schema::ArrowError,
    },
    /// Writing the append's data files failed.
    #[snafu(display("failed to write lake data files ({stage})"))]
    Write {
        /// Which writer stage failed.
        stage: &'static str,
        /// Underlying iceberg error (boxed: it is large, and errors are cold).
        #[snafu(source(from(iceberg::Error, Box::new)))]
        source: Box<iceberg::Error>,
    },
    /// Committing the append failed (conflicts are retried first).
    #[snafu(display("failed to commit the lake append after {attempts} attempt(s)"))]
    Commit {
        /// How many commit attempts were made.
        attempts: u32,
        /// Underlying iceberg error (boxed: it is large, and errors are cold).
        #[snafu(source(from(iceberg::Error, Box::new)))]
        source: Box<iceberg::Error>,
    },
    /// A data file could not be read during an incremental walk.
    #[snafu(display("failed to read lake data file {path}"))]
    ReadFile {
        /// Object path of the data file.
        path: String,
        /// Underlying iceberg error (boxed: it is large, and errors are cold).
        #[snafu(source(from(iceberg::Error, Box::new)))]
        source: Box<iceberg::Error>,
    },
    /// A data file could not be opened as parquet.
    #[snafu(display("failed to parse lake data file {path} as parquet"))]
    ParseFile {
        /// Object path of the data file.
        path: String,
        /// Underlying parquet error.
        source: parquet_57::errors::ParquetError,
    },
    /// A data file's record batches could not be decoded.
    #[snafu(display("failed to decode lake data file {path}"))]
    DecodeFile {
        /// Object path of the data file.
        path: String,
        /// Underlying arrow error.
        source: arrow_schema::ArrowError,
    },
    /// The cursor snapshot is no longer in table metadata (expired or never
    /// existed); the caller must fall back to a full rescan.
    #[snafu(display("cursor snapshot {snapshot} not found (expired?); a full rescan is required"))]
    CursorNotFound {
        /// The cursor's snapshot id.
        snapshot: i64,
    },
    /// A required column was absent from a scanned batch.
    #[snafu(display("the lake scan is missing column {column}"))]
    MissingColumn {
        /// Name of the absent column.
        column: &'static str,
    },
    /// A column was present but not the expected arrow type.
    #[snafu(display("lake column {column} has an unexpected type"))]
    ColumnType {
        /// Name of the mis-typed column.
        column: &'static str,
    },
    /// A required cell was null. The writer never produces this shape, so a
    /// null here is a malformed log.
    #[snafu(display("lake column {column} is null at row {row}"))]
    NullValue {
        /// Name of the column with the null cell.
        column: &'static str,
        /// Row index of the null cell.
        row: usize,
    },
    /// A row's `op` was neither `upsert` nor `delete`.
    #[snafu(display("unknown lake op {value:?} at row {row}"))]
    BadOp {
        /// The unexpected op value.
        value: String,
        /// Row index of the bad op.
        row: usize,
    },
    /// A row's `meta_json` did not parse as JSON.
    #[snafu(display("failed to parse meta_json from the lake"))]
    MetaJson {
        /// Underlying serde error.
        source: serde_json::Error,
    },
    /// A tombstone (or state-projected) row cannot become a
    /// [`Document`](source_meta::Document).
    #[snafu(display("cannot build a Document from a tombstone row (missing {what})"))]
    TombstoneDocument {
        /// Which content field was absent.
        what: &'static str,
    },
    /// The system clock is before the unix epoch.
    #[snafu(display("system clock is before the unix epoch"))]
    ClockBeforeEpoch {
        /// Underlying time error.
        source: std::time::SystemTimeError,
    },
    /// The system clock does not fit the `observed_at` column.
    #[snafu(display("system clock out of range for observed_at"))]
    Clock {
        /// Underlying conversion error.
        source: std::num::TryFromIntError,
    },
}

/// Result alias defaulting to this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;
