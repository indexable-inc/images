//! Python bindings for `scipql-core`, declared through `unibind`.
//!
//! The boundary keeps the same thin sync shape the hand-written `pyo3`
//! version had: [`_scipql::index`] runs `rust-analyzer scip`,
//! [`_scipql::facts`] lowers an index to relations, [`_scipql::query`] runs
//! a Soufflé program, and [`_scipql::fix`] / [`_scipql::rename`] apply edits
//! (returning the unified diff). All logic lives in the core crate; the
//! exported module only converts at the boundary, and `unibind` renders the
//! `pyo3` glue (function wrappers, record classes, the exception hierarchy,
//! and the module registration) from these declarations.

// `backends(py)`: a whole-workspace build unifies unibind's backend
// features across consumers (the ts conformance crate enables `ts`), so pin
// this crate's glue to the backend whose runtime deps it declares.
#[unibind::export(backends(py))]
mod _scipql {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// One `occurrence` fact: a symbol use site with its byte range and role.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Occurrence {
        pub symbol: String,
        pub path: String,
        pub start: usize,
        pub end: usize,
        pub role: String,
    }

    /// One `symbol_info` fact: a symbol's kind and display name.
    #[unibind::record]
    #[derive(Clone)]
    pub struct SymbolInfo {
        pub symbol: String,
        pub kind: String,
        pub display_name: String,
    }

    /// One `document` fact: an indexed source path.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Document {
        pub path: String,
    }

    /// One `relationship` fact: a typed edge between two symbols.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Relationship {
        pub symbol: String,
        pub related: String,
        pub kind: String,
    }

    /// The four fact relations a SCIP index lowers into.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Facts {
        pub occurrence: Vec<Occurrence>,
        pub symbol_info: Vec<SymbolInfo>,
        pub document: Vec<Document>,
        pub relationship: Vec<Relationship>,
    }

    /// One Soufflé `.output` relation: its column names and untyped string
    /// rows.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Relation {
        pub columns: Vec<String>,
        pub rows: Vec<HashMap<String, String>>,
    }

    /// Everything the boundary raises, split by pipeline stage. Python sees
    /// `ScipqlError` (a `ValueError`, matching the exception the hand-written
    /// binding raised) with one subclass per variant.
    #[unibind::error(py(base = "ValueError"))]
    #[derive(Debug)]
    pub enum ScipqlError {
        /// Producing, loading, or lowering the SCIP index failed.
        #[unibind(py(name = "IndexingError"))]
        Indexing { message: String },
        /// Materializing facts or running Soufflé failed.
        #[unibind(py(name = "SouffleError"))]
        Souffle { message: String },
        /// Computing or applying edits failed.
        #[unibind(py(name = "EditError"))]
        Edit { message: String },
    }

    impl std::fmt::Display for ScipqlError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let (Self::Indexing { message } | Self::Souffle { message } | Self::Edit { message }) =
                self;
            formatter.write_str(message)
        }
    }

    impl std::error::Error for ScipqlError {}

    /// Flatten a core error and its source chain into one message, matching
    /// the text the hand-written binding put on its `ValueError`.
    fn chain(error: &scipql_core::Error) -> String {
        let mut message = error.to_string();
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            message.push_str(": ");
            message.push_str(&cause.to_string());
            source = cause.source();
        }
        message
    }

    /// Sort a core error into the boundary's exception classes.
    fn classify(error: &scipql_core::Error) -> ScipqlError {
        use scipql_core::Error;
        let message = chain(error);
        match error {
            Error::RunRustAnalyzer { .. }
            | Error::RustAnalyzerFailed { .. }
            | Error::ReadIndex { .. }
            | Error::ParseIndex { .. }
            | Error::ReadSource { .. }
            | Error::Offset { .. } => ScipqlError::Indexing { message },
            Error::ReadProgram { .. }
            | Error::WriteFacts { .. }
            | Error::Scratch { .. }
            | Error::RunSouffle { .. }
            | Error::SouffleFailed { .. }
            | Error::ReadOutput { .. } => ScipqlError::Souffle { message },
            Error::BadEditRow { .. }
            | Error::EditUnknownPath { .. }
            | Error::EmptySelector
            | Error::Overlap { .. }
            | Error::WriteRewrite { .. } => ScipqlError::Edit { message },
        }
    }

    /// Run `rust-analyzer scip` on `project`, writing the index to `output`
    /// (default `index.scip`). Returns the output path.
    pub fn index(
        project: &str,
        #[unibind(default = "index.scip")] output: &str,
    ) -> Result<String, ScipqlError> {
        scipql_core::index(Path::new(project), Path::new(output))
            .map_err(|error| classify(&error))?;
        Ok(output.to_owned())
    }

    /// Lower a SCIP index into the four fact relations.
    ///
    /// `root` resolves relative document paths for byte offsets; it defaults
    /// to the index's project root.
    pub fn facts(index_path: &str, root: Option<&str>) -> Result<Facts, ScipqlError> {
        let loaded =
            scipql_core::load_index(Path::new(index_path)).map_err(|error| classify(&error))?;
        let root = root.map(PathBuf::from);
        let facts = scipql_core::facts_from_index(&loaded, root.as_deref())
            .map_err(|error| classify(&error))?;
        Ok(Facts {
            occurrence: facts
                .occurrences
                .into_iter()
                .map(|row| Occurrence {
                    symbol: row.symbol,
                    path: row.path,
                    start: row.start,
                    end: row.end,
                    role: row.role,
                })
                .collect(),
            symbol_info: facts
                .symbols
                .into_iter()
                .map(|row| SymbolInfo {
                    symbol: row.symbol,
                    kind: row.kind,
                    display_name: row.display_name,
                })
                .collect(),
            document: facts
                .documents
                .into_iter()
                .map(|path| Document { path })
                .collect(),
            relationship: facts
                .relationships
                .into_iter()
                .map(|row| Relationship {
                    symbol: row.symbol,
                    related: row.related,
                    kind: row.kind,
                })
                .collect(),
        })
    }

    /// Run a Soufflé `program` over the index's facts. Returns one relation
    /// per `.output` declaration, keyed by relation name. The fact relations
    /// are already in scope.
    pub fn query(
        index_path: &str,
        program: &str,
        root: Option<&str>,
    ) -> Result<HashMap<String, Relation>, ScipqlError> {
        let loaded =
            scipql_core::load_index(Path::new(index_path)).map_err(|error| classify(&error))?;
        let root = root.map(PathBuf::from);
        let output = scipql_core::query(&loaded, root.as_deref(), program)
            .map_err(|error| classify(&error))?;
        Ok(output
            .relations
            .into_iter()
            .map(|relation| {
                let rows = relation
                    .rows
                    .into_iter()
                    .map(|row| relation.columns.iter().cloned().zip(row).collect())
                    .collect();
                (
                    relation.name,
                    Relation {
                        columns: relation.columns,
                        rows,
                    },
                )
            })
            .collect())
    }

    /// Run a `fix` program (one that `.output`s `edit(path, start, end,
    /// replacement)`) and return the unified diff. With `write=True` the
    /// files under `root` are rewritten on disk.
    pub fn fix(
        index_path: &str,
        program: &str,
        root: Option<&str>,
        #[unibind(default = false)] write: bool,
    ) -> Result<String, ScipqlError> {
        let loaded =
            scipql_core::load_index(Path::new(index_path)).map_err(|error| classify(&error))?;
        let root = root.map(PathBuf::from);
        scipql_core::fix(&loaded, root.as_deref(), program, write)
            .map_err(|error| classify(&error))
    }

    /// Rename every occurrence whose SCIP moniker ends with `selector` to
    /// `new_name`. Returns the unified diff; `write=True` applies it.
    pub fn rename(
        index_path: &str,
        selector: &str,
        new_name: &str,
        root: Option<&str>,
        #[unibind(default = false)] write: bool,
    ) -> Result<String, ScipqlError> {
        let loaded =
            scipql_core::load_index(Path::new(index_path)).map_err(|error| classify(&error))?;
        let root = root.map(PathBuf::from);
        scipql_core::rename(&loaded, root.as_deref(), selector, new_name, write)
            .map_err(|error| classify(&error))
    }
}
