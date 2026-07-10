/// A sample boundary exercising the phase 0-2 surface.
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

    /// Wait for one row.
    pub async fn fetch_row(
        id: u64,
        #[unibind(default = 250)] timeout_ms: u64,
    ) -> Result<Row, SampleError> {
        let _ = (id, timeout_ms);
        Err(SampleError::Invalid(String::new()))
    }

    /// Snapshot the head row.
    pub async fn head() -> Row {
        todo!()
    }

    /// Checksum data off the GIL.
    #[unibind(blocking)]
    pub fn digest(data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }

    /// Tick forever.
    pub fn ticks(period_ms: u64) -> UniStream<u64> {
        let _ = period_ms;
        todo!()
    }

    /// Follow rows as they land.
    pub async fn follow(store: String) -> Result<UniStream<Row>, SampleError> {
        let _ = store;
        todo!()
    }

    /// A live store handle.
    #[unibind::object(resource)]
    pub struct Store {
        rows: u64,
    }

    impl Store {
        /// Open a store.
        #[unibind(constructor)]
        pub fn open(path: &str) -> Result<Self, SampleError> {
            let _ = path;
            Err(SampleError::Invalid(String::new()))
        }

        /// Count rows.
        pub fn len(&self) -> u64 {
            self.rows
        }

        /// Pull one row.
        pub async fn get(&self, id: u64) -> Result<Row, SampleError> {
            let _ = id;
            Err(SampleError::Invalid(String::new()))
        }

        /// Flush to disk.
        #[unibind(py(name = "sync_all"))]
        pub fn sync(&self) -> bool {
            true
        }

        /// Release the store.
        pub async fn close(&self) -> Result<(), SampleError> {
            Ok(())
        }
    }

    /// A cursor over rows.
    #[unibind::object]
    pub struct Cursor {
        position: u64,
    }

    impl Cursor {
        /// Step forward.
        pub fn advance(&self, by: u64) -> u64 {
            self.position + by
        }
    }

    /// Open a cursor.
    pub async fn cursor() -> Cursor {
        Cursor { position: 0 }
    }
}
