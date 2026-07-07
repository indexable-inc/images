//! Conformance surface for the unibind JVM backend (phase 8, #2083).
//!
//! The sync phase-0 subset of the Python suite one directory up
//! (`packages/unibind/conformance`), under the same case names, plus width
//! and container coverage the JVM ABI needs proven: every integer width,
//! both floats, borrowed and owned text/paths/bytes, `Option` in both
//! states, containers (empty and full), record round-trips with nested
//! aggregates, every default-literal kind, the exception hierarchy, and
//! panic containment. `runner/Main.java` and `runner/main.kt` drive each
//! export through the generated Panama binding; the async/stream/object
//! cases join when the JVM async surface lands (#2083 phase D).

/// The exported boundary: Java package `unibind.conformance`, loaded via
/// the `unibind.conformance.library` system property.
// The exported signatures ARE the conformance cases: owned arguments and
// non-const bodies are the shapes being proven across the boundary, so the
// perf-shape lints do not apply to them.
#[allow(clippy::missing_const_for_fn, clippy::needless_pass_by_value)]
#[unibind::export(backends(jvm))]
mod conformance {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// A plain-data record crossing the boundary by value.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Point {
        /// Horizontal coordinate.
        pub x: f64,
        /// Vertical coordinate.
        pub y: f64,
    }

    /// A record with nested aggregates, proving container fields round-trip.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Row {
        /// Identifier.
        pub id: u64,
        /// Display label.
        pub label: String,
        /// Plain string list.
        pub tags: Vec<String>,
        /// String-keyed float map.
        pub scores: HashMap<String, f64>,
        /// Raw payload bytes.
        pub blob: Vec<u8>,
        /// Optional origin path.
        pub origin: Option<PathBuf>,
    }

    /// Boundary failures raised by the conformance surface.
    #[unibind::error]
    #[derive(Debug)]
    pub enum ConformanceError {
        /// A deliberate failure for exception-mapping tests.
        Deliberate { message: String },
        /// A missing name, proving status N maps onto the N-th variant.
        Missing { name: String },
    }

    impl std::fmt::Display for ConformanceError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Deliberate { message } => write!(formatter, "{message}"),
                Self::Missing { name } => write!(formatter, "no such name: {name}"),
            }
        }
    }

    impl std::error::Error for ConformanceError {}

    /// Round-trip a bool.
    pub fn echo_bool(value: bool) -> bool {
        value
    }

    /// Round-trip an int.
    pub fn echo_int(value: i64) -> i64 {
        value
    }

    /// Round-trip an `i8`.
    pub fn echo_i8(value: i8) -> i8 {
        value
    }

    /// Round-trip an `i16`.
    pub fn echo_i16(value: i16) -> i16 {
        value
    }

    /// Round-trip an `i32`.
    pub fn echo_i32(value: i32) -> i32 {
        value
    }

    /// Round-trip a `u8`.
    pub fn echo_u8(value: u8) -> u8 {
        value
    }

    /// Round-trip a `u16`.
    pub fn echo_u16(value: u16) -> u16 {
        value
    }

    /// Round-trip a `u32`.
    pub fn echo_u32(value: u32) -> u32 {
        value
    }

    /// Round-trip a `u64`; `u64::MAX` reads back as `-1` bits in Java.
    pub fn echo_u64(value: u64) -> u64 {
        value
    }

    /// Round-trip a `usize`.
    pub fn echo_usize(value: usize) -> usize {
        value
    }

    /// Round-trip an `isize`.
    pub fn echo_isize(value: isize) -> isize {
        value
    }

    /// Round-trip an `f32`.
    pub fn echo_f32(value: f32) -> f32 {
        value
    }

    /// Round-trip a float.
    pub fn echo_float(value: f64) -> f64 {
        value
    }

    /// Round-trip a string; unicode crosses as UTF-8.
    pub fn echo_str(value: String) -> String {
        value
    }

    /// Byte length of a borrowed string view.
    pub fn str_len(value: &str) -> usize {
        value.len()
    }

    /// Round-trip an owned path.
    pub fn echo_path(value: PathBuf) -> PathBuf {
        value
    }

    /// Join a borrowed path with a child segment.
    pub fn path_join(base: &Path, child: String) -> PathBuf {
        base.join(child)
    }

    /// Round-trip bytes; the argument view is copied into an owned return.
    pub fn echo_bytes(data: &[u8]) -> Vec<u8> {
        data.to_vec()
    }

    /// Reverse owned bytes, proving `Vec<u8>` crosses in both directions.
    pub fn reverse_bytes(data: Vec<u8>) -> Vec<u8> {
        data.into_iter().rev().collect()
    }

    /// Round-trip an optional int.
    pub fn echo_option(value: Option<i64>) -> Option<i64> {
        value
    }

    /// Round-trip an optional string.
    pub fn echo_option_str(value: Option<String>) -> Option<String> {
        value
    }

    /// Round-trip a list of ints.
    pub fn echo_vec(values: Vec<i64>) -> Vec<i64> {
        values
    }

    /// Round-trip a list of strings.
    pub fn echo_vec_str(values: Vec<String>) -> Vec<String> {
        values
    }

    /// Round-trip a string-keyed map of floats.
    pub fn echo_map(values: HashMap<String, f64>) -> HashMap<String, f64> {
        values
    }

    /// Round-trip a record.
    pub fn echo_record(point: Point) -> Point {
        point
    }

    /// Round-trip a record with nested aggregate fields.
    pub fn echo_row(row: Row) -> Row {
        row
    }

    /// Round-trip a list of records.
    pub fn echo_rows(rows: Vec<Row>) -> Vec<Row> {
        rows
    }

    /// Build a row from parts: aggregates in argument position, a record in
    /// return position.
    pub fn make_row(
        id: u64,
        label: String,
        tags: Vec<String>,
        scores: HashMap<String, f64>,
        blob: Vec<u8>,
        origin: Option<PathBuf>,
    ) -> Row {
        Row {
            id,
            label,
            tags,
            scores,
            blob,
            origin,
        }
    }

    /// Add with a defaulted second operand, proving `#[unibind(default)]`.
    pub fn add_with_default(value: i64, #[unibind(default = 32)] delta: i64) -> i64 {
        value + delta
    }

    /// Format a greeting through every default-literal kind: float, string,
    /// bool, and the implicit `None` of a defaultless `Option`.
    pub fn greet(
        name: String,
        #[unibind(default = 1.5)] ratio: f64,
        #[unibind(default = "friend")] title: &str,
        #[unibind(default = true)] excited: bool,
        note: Option<String>,
    ) -> String {
        let punctuation = if excited { "!" } else { "." };
        let suffix = note.map(|text| format!(" ({text})")).unwrap_or_default();
        format!("{title} {name} x{ratio}{punctuation}{suffix}")
    }

    /// Raise the first error variant.
    ///
    /// # Errors
    ///
    /// Always: proving the enum maps onto the exception hierarchy is the
    /// point.
    pub fn throw_value_error() -> Result<(), ConformanceError> {
        Err(ConformanceError::Deliberate {
            message: "conformance deliberate failure".to_owned(),
        })
    }

    /// Raise the second error variant through a value-returning signature.
    ///
    /// # Errors
    ///
    /// Always, with the variant carrying `name`.
    pub fn throw_missing(name: String) -> Result<i64, ConformanceError> {
        Err(ConformanceError::Missing { name })
    }

    /// Add through a fallible signature, exercising the `Ok` path.
    ///
    /// # Errors
    ///
    /// On `i64` overflow.
    pub fn checked_add(a: i64, b: i64) -> Result<i64, ConformanceError> {
        a.checked_add(b).ok_or_else(|| ConformanceError::Deliberate {
            message: "i64 overflow".to_owned(),
        })
    }

    /// Panic synchronously.
    ///
    /// # Panics
    ///
    /// Always: proving panics surface as `UnibindPanicException` without
    /// killing the JVM is the point.
    pub fn panic_sync() {
        panic!("unibind conformance: deliberate sync panic");
    }
}
