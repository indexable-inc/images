/// A sample boundary exercising the elixir surface.
mod sample {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use unibind_runtime::UniStream;

    /// A row.
    #[unibind::record(ex(name = "Line"))]
    #[derive(Clone)]
    pub struct Row {
        /// Identifier.
        pub id: u64,
        pub name: String,
        pub weights: HashMap<String, f64>,
        pub home: Option<PathBuf>,
    }

    /// Boundary failures.
    #[unibind::error(py(base = "RuntimeError"), ex(name = "SampleFault"))]
    pub enum SampleError {
        /// The store is gone.
        #[unibind(ex(name = "MissingStore"))]
        StoreGone { message: String },
        /// Bad input.
        Invalid(String),
    }

    /// Fetch rows.
    ///
    /// Docs become `@doc`s.
    pub fn rows(
        store: &str,
        #[unibind(default = 10)] limit: usize,
        root: Option<&str>,
    ) -> Result<Vec<Row>, SampleError> {
        let _ = (store, limit, root);
        Ok(Vec::new())
    }

    /// Recount everything; long-running, so scheduled dirty.
    #[unibind(blocking)]
    pub fn recount(home: PathBuf) -> u64 {
        let _ = home;
        0
    }

    /// Resolve a label off the scheduler.
    #[unibind(ex(name = "label_of"))]
    pub async fn label(#[unibind(ex(name = "key"))] id: u64, prefix: String) -> String {
        format!("{prefix}{id}")
    }

    /// Persist a row.
    pub async fn store(row: Row) -> Result<(), SampleError> {
        let _ = row;
        Ok(())
    }

    /// Every tag, on demand.
    pub fn tags(prefix: &str) -> UniStream<String> {
        let _ = prefix;
        UniStream::new(futures::stream::iter(Vec::new()))
    }

    /// Stream rows, verifying the store first.
    pub fn scan(store: &str) -> Result<UniStream<Row>, SampleError> {
        let _ = store;
        Ok(UniStream::new(futures::stream::iter(Vec::new())))
    }

    /// A live cursor.
    #[unibind::object]
    pub struct Cursor {
        position: u64,
    }

    impl Cursor {
        /// Open at the start.
        #[unibind(constructor)]
        pub fn open(store: &str) -> Result<Self, SampleError> {
            let _ = store;
            Ok(Self { position: 0 })
        }

        /// The current position.
        pub fn position(&self) -> u64 {
            self.position
        }

        /// Skip ahead; long-running, so scheduled dirty.
        #[unibind(blocking)]
        pub fn skip(&self, n: u64) -> u64 {
            self.position + n
        }
    }
}
