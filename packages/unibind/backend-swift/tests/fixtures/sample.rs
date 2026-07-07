/// A sample boundary exercising the swift backend surface.
mod sample {
    use std::collections::HashMap;
    use std::path::Path;
    use std::path::PathBuf;

    /// A row.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Row {
        /// Identifier.
        pub id: u64,
        pub name: String,
        pub tags: Vec<String>,
        pub weights: HashMap<String, f64>,
        pub blob: Vec<u8>,
        pub home: Option<PathBuf>,
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
    /// Docs become documentation comments.
    pub fn rows(
        store: &str,
        #[unibind(default = 10)] limit: usize,
        root: Option<&Path>,
    ) -> Result<Vec<Row>, SampleError> {
        let _ = (store, limit, root);
        Ok(Vec::new())
    }

    /// Echo the weights, sorted by key on the way back.
    pub fn weights_echo(weights: HashMap<String, f64>) -> HashMap<String, f64> {
        weights
    }

    pub fn touch(
        path: &Path,
        data: &[u8],
        #[unibind(default = 0.5)] ratio: f64,
        #[unibind(default = "note")] note: &str,
        #[unibind(default = false)] flush: bool,
    ) -> bool {
        let _ = (path, data, ratio, note, flush);
        true
    }

    /// The first row, if any.
    pub fn first(rows: Vec<Row>) -> Option<Row> {
        rows.into_iter().next()
    }

    /// Echo an optional label.
    pub fn echo_option_string(value: Option<String>) -> Option<String> {
        value
    }

    /// Echo a nested composition: map values that are vectors.
    pub fn series(table: HashMap<String, Vec<f64>>) -> HashMap<String, Vec<f64>> {
        table
    }

    pub fn echo_isize(value: isize) -> isize {
        value
    }

    pub fn count(rows: Vec<Row>) -> usize {
        rows.len()
    }

    /// Echo bytes by value.
    pub fn echo_bytes(value: Vec<u8>) -> Vec<u8> {
        value
    }

    /// Echo a float.
    pub fn echo_f32(value: f32) -> f32 {
        value
    }
}
