/// A sample boundary exercising the jvm surface.
mod _sample {
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// A row.
    #[unibind::record(jvm(name = "Line"))]
    #[derive(Clone)]
    pub struct Row {
        /// Identifier.
        pub id: u64,
        pub name: String,
        pub weights: HashMap<String, f64>,
        pub home: Option<PathBuf>,
    }

    /// Boundary failures.
    #[unibind::error(py(base = "RuntimeError"), jvm(base = "IllegalStateException"))]
    pub enum SampleError {
        /// The store is gone.
        #[unibind(jvm(name = "MissingStoreException"))]
        StoreGone { message: String },
        /// Bad input.
        Invalid(String),
    }

    /// Fetch rows.
    ///
    /// Docs become javadoc.
    // clone:ignore -- fixtures deliberately parallel the other backends'
    // (../../../backend-ex, ../../../backend-py) so snapshots stay
    // comparable; syn drops line comments, so the IR is unaffected.
    pub fn rows(
        store: &str,
        #[unibind(default = 10)] limit: usize,
        root: Option<&str>,
    ) -> Result<Vec<Row>, SampleError> {
        let _ = (store, limit, root);
        Ok(Vec::new())
    }

    /// Persist a row under a path, returning the bytes written.
    pub fn store(home: PathBuf, row: Row, payload: Vec<u8>) -> u64 {
        let _ = (home, row, payload);
        0
    }

    /// Resolve a label.
    #[unibind(jvm(name = "label_of"))]
    pub fn label(
        #[unibind(jvm(name = "key"))] id: u64,
        #[unibind(default = "row-")] prefix: String,
        #[unibind(default = true)] trim: bool,
    ) -> String {
        let _ = trim;
        format!("{prefix}{id}")
    }

    /// Drop everything.
    pub fn clear() -> Result<(), SampleError> {
        Ok(())
    }
}
