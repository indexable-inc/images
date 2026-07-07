/// A sample boundary exercising the Rust backend surface.
mod sample {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use unibind_stream::UniStream;

    /// A row. The `flag`-first field order is deliberate: Rust would pack
    /// this struct tighter reordered, which exercises the mirror's layout
    /// opt-out.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Row {
        /// Awkwardly-placed bool.
        pub flag: bool,
        /// Identifier.
        pub id: u64,
        pub name: String,
        pub tags: Vec<String>,
        pub weights: HashMap<String, f64>,
        pub blob: Vec<u8>,
        pub home: Option<PathBuf>,
        pub nested: Option<Vec<Inner>>,
        pub inner: Inner,
    }

    /// A nested record.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Inner {
        pub label: String,
        pub ratio: f64,
    }

    /// Boundary failures.
    #[unibind::error]
    pub enum SampleError {
        /// The store is gone.
        StoreGone { message: String },
        /// Bad input.
        Invalid(String),
    }

    /// Fetch rows.
    ///
    /// Docs travel into the generated client.
    pub fn rows(store: &str, limit: usize, root: Option<&str>) -> Result<Vec<Row>, SampleError> {
        let _ = (store, limit, root);
        Ok(Vec::new())
    }

    /// Touch a path.
    pub fn touch(path: &std::path::Path, data: &[u8], ratio: f64) -> bool {
        let _ = (path, data, ratio);
        true
    }

    /// Reset a counter.
    pub fn reset() {}

    /// Double after yielding.
    pub async fn delayed_double(x: i64) -> i64 {
        x * 2
    }

    /// An async call that can fail.
    pub async fn fetch_row(name: String) -> Result<Row, SampleError> {
        let _ = name;
        Err(SampleError::Invalid("nope".to_owned()))
    }

    /// A stream of labels.
    pub fn labels(prefix: String) -> UniStream<String> {
        let _ = prefix;
        make_labels()
    }
}
