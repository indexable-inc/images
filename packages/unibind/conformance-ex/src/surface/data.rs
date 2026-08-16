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
    /// Optional field, `nil` on the Elixir side when absent.
    pub home: Option<String>,
}

/// Boundary failures raised by the conformance surface.
#[unibind::error]
#[derive(Debug)]
pub enum ConformanceError {
    /// A deliberate failure for error-term tests.
    Deliberate { message: String },
    /// A second variant, proving variant atoms map one to one.
    Gone { message: String },
}

unibind_ex_runtime::message_error!(ConformanceError { Deliberate, Gone });
