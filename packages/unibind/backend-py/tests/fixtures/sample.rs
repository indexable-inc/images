/// A sample boundary exercising the phase 0 surface.
mod sample {
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// A row.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Row {
        /// Identifier.
        pub id: u64,
        #[unibind(py(name = "label"))]
        pub name: String,
        pub tags: Vec<String>,
        pub weights: HashMap<String, f64>,
        pub blob: Vec<u8>,
        pub home: Option<PathBuf>,
    }

    /// Boundary failures.
    #[unibind::error(py(base = "RuntimeError"))]
    pub enum SampleError {
        /// The store is gone.
        #[unibind(py(name = "StoreGoneError"))]
        StoreGone { message: String },
        /// Bad input.
        Invalid(String),
    }

    /// Fetch rows.
    ///
    /// Docs become docstrings.
    pub fn rows(
        store: &str,
        #[unibind(default = 10)] limit: usize,
        root: Option<&str>,
    ) -> Result<Vec<Row>, SampleError> {
        let _ = (store, limit, root);
        Ok(Vec::new())
    }

    #[unibind(py(name = "touch_path"))]
    pub fn touch(
        path: &std::path::Path,
        data: &[u8],
        #[unibind(default = 0.5)] ratio: f64,
        #[unibind(default = "note")] note: &str,
        #[unibind(default = false)] flush: bool,
    ) -> bool {
        let _ = (path, data, ratio, note, flush);
        true
    }
}
