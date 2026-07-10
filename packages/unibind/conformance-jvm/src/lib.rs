//! Conformance surface for the unibind jvm backend.
//!
//! Every export exists so the JUnit-less Java suite in `java/` can prove one
//! boundary behavior from the JVM: primitives and containers round-trip the
//! wire at every width, unsigned Rust integers land reinterpreted at the
//! same width, records cross by value through their canonical constructors,
//! a declared error rebuilds as its variant's exception subclass, a panic
//! surfaces as `PanicException` instead of tearing the process down, and
//! defaulted arguments gain delegating overloads.

#![allow(
    clippy::needless_pass_by_value,
    reason = "arguments cross the jvm boundary owned; the echo surface exists to round-trip them"
)]
#![allow(
    clippy::missing_const_for_fn,
    reason = "the echo bodies are trivially const, but they stand in for real exports, which are not"
)]

/// The exported boundary. The module name names the generated Java class
/// (`UnibindConformanceJvm`) and the library key the class resolves at
/// load time (`unibind_conformance_jvm`).
#[unibind::export(backends(jvm))]
mod _unibind_conformance_jvm {
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// A plain-data record crossing the boundary by value.
    #[unibind::record]
    #[derive(Clone)]
    pub struct Sample {
        /// Stable identifier.
        pub id: u64,
        /// Display name.
        pub name: String,
        /// A fraction, proving floats survive the struct codec.
        pub ratio: f64,
        /// Nested list field.
        pub tags: Vec<String>,
        /// Optional field, `null` on the Java side when absent.
        pub home: Option<String>,
    }

    /// Boundary failures raised by the conformance surface.
    #[unibind::error]
    #[derive(Debug)]
    pub enum ConformanceError {
        /// A deliberate failure for exception-mapping tests.
        Deliberate { message: String },
        /// A second variant, proving subclasses map one to one.
        Gone { message: String },
    }

    impl std::fmt::Display for ConformanceError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Deliberate { message } | Self::Gone { message } => {
                    write!(formatter, "{message}")
                }
            }
        }
    }

    impl std::error::Error for ConformanceError {}

    /// Round-trip a bool.
    pub fn echo_bool(value: bool) -> bool {
        value
    }

    /// Round-trip a signed byte.
    pub fn echo_byte(value: i8) -> i8 {
        value
    }

    /// Round-trip a signed short.
    pub fn echo_short(value: i16) -> i16 {
        value
    }

    /// Round-trip a signed int.
    pub fn echo_int(value: i32) -> i32 {
        value
    }

    /// Round-trip a signed long.
    pub fn echo_long(value: i64) -> i64 {
        value
    }

    /// Round-trip an unsigned int; Java sees the same 32 bits as a signed
    /// `int`, so `u32::MAX` lands as `-1`.
    pub fn echo_uint(value: u32) -> u32 {
        value
    }

    /// Round-trip an unsigned long; the same 64-bit reinterpretation.
    pub fn echo_ulong(value: u64) -> u64 {
        value
    }

    /// Round-trip a 32-bit float.
    pub fn echo_float(value: f32) -> f32 {
        value
    }

    /// Round-trip a 64-bit float.
    pub fn echo_double(value: f64) -> f64 {
        value
    }

    /// Round-trip a string.
    pub fn echo_str(value: String) -> String {
        value
    }

    /// Round-trip binary data.
    pub fn echo_bytes(value: Vec<u8>) -> Vec<u8> {
        value
    }

    /// Round-trip a filesystem path; it crosses the wire as a string.
    pub fn echo_path(value: PathBuf) -> PathBuf {
        value
    }

    /// Round-trip an optional string; `null` crosses as `None`.
    pub fn echo_option(value: Option<String>) -> Option<String> {
        value
    }

    /// Round-trip a list of longs.
    pub fn echo_vec(values: Vec<i64>) -> Vec<i64> {
        values
    }

    /// Round-trip a string-keyed map of longs.
    pub fn echo_map(values: HashMap<String, i64>) -> HashMap<String, i64> {
        values
    }

    /// Round-trip a record struct.
    pub fn echo_record(sample: Sample) -> Sample {
        sample
    }

    /// Round-trip a nested list of records.
    pub fn echo_records(samples: Vec<Sample>) -> Vec<Sample> {
        samples
    }

    /// Ok or the `Deliberate` exception subclass, by input.
    ///
    /// # Errors
    ///
    /// When `fail` is true; proving the variant-index envelope rebuilds the
    /// right subclass is the point.
    pub fn maybe_fail(fail: bool) -> Result<i64, ConformanceError> {
        if fail {
            return Err(ConformanceError::Deliberate {
                message: "conformance deliberate failure".to_owned(),
            });
        }
        Ok(42)
    }

    /// Always the `Gone` exception subclass.
    ///
    /// # Errors
    ///
    /// Always; proving the second variant maps to its own subclass.
    pub fn lost() -> Result<i64, ConformanceError> {
        Err(ConformanceError::Gone {
            message: "conformance gone failure".to_owned(),
        })
    }

    /// Greet `name`, with the greeting and punctuation defaulted; the
    /// generated class carries one delegating overload per dropped
    /// trailing argument.
    pub fn greet(
        name: String,
        #[unibind(default = "hello")] greeting: String,
        #[unibind(default = 1)] exclamations: u32,
    ) -> String {
        let marks = "!".repeat(usize::try_from(exclamations).expect("u32 fits usize"));
        format!("{greeting}, {name}{marks}")
    }

    /// Panic across the boundary.
    ///
    /// # Panics
    ///
    /// Always; the envelope must surface it as `PanicException` rather
    /// than unwinding into the JVM.
    pub fn explode() {
        panic!("conformance deliberate panic");
    }
}
