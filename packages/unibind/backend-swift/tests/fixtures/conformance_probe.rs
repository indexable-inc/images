// A snapshot of packages/unibind/conformance-swift/src/lib.rs's exported
// module (attribute dropped), so the probe test exercises swift-bridge's
// parser and codegen over the exact conformance surface. Refresh it when
// the conformance crate's surface changes.
pub mod _conformance {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// A plain data row crossing the boundary by value.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Row {
        /// Identifier.
        pub id: u64,
        /// Display label.
        pub label: String,
        pub tags: Vec<String>,
        pub weights: HashMap<String, f64>,
        pub blob: Vec<u8>,
        pub home: Option<PathBuf>,
    }

    /// Everything the conformance boundary raises.
    #[unibind::error]
    pub enum ConformanceError {
        /// The store is gone.
        StoreGone { store: String },
        /// Bad input.
        Invalid(String),
    }

    impl std::fmt::Display for ConformanceError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::StoreGone { store } => write!(formatter, "store `{store}` is gone"),
                Self::Invalid(reason) => write!(formatter, "invalid input: {reason}"),
            }
        }
    }

    /// Echo a bool.
    pub fn echo_bool(value: bool) -> bool {
        value
    }

    pub fn echo_i8(value: i8) -> i8 {
        value
    }

    pub fn echo_i16(value: i16) -> i16 {
        value
    }

    pub fn echo_i32(value: i32) -> i32 {
        value
    }

    pub fn echo_i64(value: i64) -> i64 {
        value
    }

    pub fn echo_isize(value: isize) -> isize {
        value
    }

    pub fn echo_u8(value: u8) -> u8 {
        value
    }

    pub fn echo_u16(value: u16) -> u16 {
        value
    }

    pub fn echo_u32(value: u32) -> u32 {
        value
    }

    pub fn echo_u64(value: u64) -> u64 {
        value
    }

    pub fn echo_usize(value: usize) -> usize {
        value
    }

    pub fn echo_f32(value: f32) -> f32 {
        value
    }

    pub fn echo_f64(value: f64) -> f64 {
        value
    }

    /// Echo an owned string.
    pub fn echo_string(value: String) -> String {
        value
    }

    /// Greet through a borrowed string.
    pub fn greet(name: &str) -> String {
        format!("hello {name}")
    }

    /// Echo an owned path (carried as UTF-8 text at the boundary).
    pub fn echo_path(value: PathBuf) -> PathBuf {
        value
    }

    /// The number of components of a borrowed path.
    pub fn path_components(path: &Path) -> usize {
        path.components().count()
    }

    /// Echo owned bytes.
    pub fn echo_bytes(value: Vec<u8>) -> Vec<u8> {
        value
    }

    /// Sum borrowed bytes.
    pub fn byte_sum(data: &[u8]) -> u64 {
        data.iter().map(|byte| u64::from(*byte)).sum()
    }

    /// Echo an optional integer.
    pub fn echo_option_i64(value: Option<i64>) -> Option<i64> {
        value
    }

    /// Echo an optional string.
    pub fn echo_option_string(value: Option<String>) -> Option<String> {
        value
    }

    /// Echo a vector of integers.
    pub fn echo_vec_i64(value: Vec<i64>) -> Vec<i64> {
        value
    }

    /// Echo a vector of strings (an opaque box at the bridge).
    pub fn echo_vec_string(value: Vec<String>) -> Vec<String> {
        value
    }

    /// Echo a map (returned entries iterate sorted by key).
    pub fn echo_map(value: HashMap<String, i64>) -> HashMap<String, i64> {
        value
    }

    /// Echo a nested composition: map values that are vectors.
    pub fn echo_map_of_vec(value: HashMap<String, Vec<f64>>) -> HashMap<String, Vec<f64>> {
        value
    }

    /// Echo a record.
    pub fn echo_row(row: Row) -> Row {
        row
    }

    /// Echo a vector of records.
    pub fn echo_rows(rows: Vec<Row>) -> Vec<Row> {
        rows
    }

    /// The first row, if any (an optional record crossing back).
    pub fn first_row(rows: Vec<Row>) -> Option<Row> {
        rows.into_iter().next()
    }

    /// Fail with `StoreGone` when `trigger` is set.
    ///
    /// The thrown Swift error carries the variant and the `Display` text.
    pub fn fail_if(trigger: bool, store: &str) -> Result<i64, ConformanceError> {
        if trigger {
            return Err(ConformanceError::StoreGone {
                store: store.to_owned(),
            });
        }
        Ok(41)
    }

    /// Integer division that throws `Invalid` on a zero divisor.
    pub fn checked_div(dividend: i64, divisor: i64) -> Result<i64, ConformanceError> {
        if divisor == 0 {
            return Err(ConformanceError::Invalid("division by zero".to_owned()));
        }
        Ok(dividend / divisor)
    }

    /// Repeat a word, with default arguments rendered into the Swift
    /// signature.
    pub fn repeat(
        word: &str,
        #[unibind(default = 3)] count: usize,
        #[unibind(default = " ")] separator: &str,
    ) -> String {
        vec![word; count].join(separator)
    }
}
