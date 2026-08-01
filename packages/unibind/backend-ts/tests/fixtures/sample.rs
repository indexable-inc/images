/// A sample boundary exercising the ts surface.
mod sample_ts {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

    /// A row.
    #[unibind::record(ts(name = "SampleRow"))]
    #[derive(Clone)]
    pub struct Row {
        /// Identifier.
        pub id: i64,
        #[unibind(ts(name = "rowLabel"), py(name = "label"))]
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
        #[unibind(ts(name = "StoreGoneError"))]
        StoreGone { message: String },
        /// Bad input.
        Invalid(String),
    }

    impl std::fmt::Display for SampleError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::StoreGone { message } => write!(formatter, "store gone: {message}"),
                Self::Invalid(input) => write!(formatter, "bad input: {input}"),
            }
        }
    }

    /// Fetch rows.
    ///
    /// Docs reach the generated `.d.ts`.
    pub fn rows(
        store: &str,
        #[unibind(default = 10)] limit: u32,
        root: Option<&str>,
    ) -> Result<Vec<Row>, SampleError> {
        let _ = (store, limit, root);
        Ok(Vec::new())
    }

    #[unibind(ts(name = "touchPath"))]
    pub fn touch(
        path: &std::path::Path,
        data: &[u8],
        #[unibind(default = 0.5)] ratio: f64,
        #[unibind(default = "note")] note: &str,
    ) -> bool {
        let _ = (path, data, ratio, note);
        true
    }

    /// Wrapping byte sum; `blocking` frees Python's GIL and renders as a
    /// plain sync export for JavaScript.
    #[unibind(blocking)]
    pub fn checksum(data: &[u8]) -> u32 {
        data.iter().fold(0, |acc, byte| acc.wrapping_add(u32::from(*byte)))
    }

    /// Add, slowly.
    pub async fn slow_add(a: i64, b: i64) -> i64 {
        a + b
    }

    /// Fetch one row.
    pub async fn fetch(store: String) -> Result<Row, SampleError> {
        Err(SampleError::Invalid(store))
    }

    /// Tail rows as a pull stream.
    pub fn tail(store: &str) -> unibind_runtime::UniStream<Row> {
        let _ = store;
        unibind_runtime::UniStream::new(futures::stream::iter(Vec::new()))
    }

    /// Tail rows once the store opens (an async stream function).
    pub async fn tail_later(store: String) -> Result<unibind_runtime::UniStream<Row>, SampleError> {
        let _ = store;
        Ok(unibind_runtime::UniStream::new(futures::stream::iter(Vec::new())))
    }

    /// A counter resource.
    #[unibind::object(resource)]
    pub struct Counter {
        total: AtomicI64,
        open: AtomicBool,
    }

    impl Counter {
        /// Open a counter.
        #[unibind(constructor)]
        pub fn new(#[unibind(default = 0)] start: i64) -> Result<Self, SampleError> {
            if start < 0 {
                return Err(SampleError::Invalid(start.to_string()));
            }
            Ok(Self {
                total: AtomicI64::new(start),
                open: AtomicBool::new(true),
            })
        }

        /// Current value.
        pub fn value(&self) -> i64 {
            self.total.load(Ordering::Relaxed)
        }

        /// Add and return the new value.
        #[unibind(ts(name = "addSlowly"))]
        pub async fn add(&self, amount: i64) -> Result<i64, SampleError> {
            Ok(self.total.fetch_add(amount, Ordering::Relaxed) + amount)
        }

        /// Release the counter.
        pub async fn close(&self) {
            self.open.store(false, Ordering::SeqCst);
        }
    }

    /// Open a counter from a free function (the non-constructor path).
    pub fn open_counter(start: i64) -> Counter {
        Counter {
            total: AtomicI64::new(start),
            open: AtomicBool::new(true),
        }
    }
}
