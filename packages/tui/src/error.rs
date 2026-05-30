use snafu::Snafu;
use uuid::Uuid;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(
        display("Failed to spawn process '{command}': {source}"),
        visibility(pub(crate))
    )]
    ProcessSpawn {
        command: String,
        source: std::io::Error,
    },

    #[snafu(display("TUI instance {id} not found"))]
    TuiNotFound { id: Uuid },

    #[snafu(
        display("Failed to write to TUI {id}: {source}"),
        visibility(pub(crate))
    )]
    WriteToTui { id: Uuid, source: std::io::Error },

    #[snafu(display("Failed to read from TUI {id}: {source}"))]
    ReadFromTui { id: Uuid, source: std::io::Error },

    #[snafu(display("Failed to signal TUI {id}: {source}"), visibility(pub(crate)))]
    SignalTui { id: Uuid, source: std::io::Error },

    #[snafu(display("Failed to resize TUI {id}: {source}"), visibility(pub(crate)))]
    ResizeTui { id: Uuid, source: std::io::Error },

    #[snafu(display("TUI {id} has no buffered output available"))]
    NoOutputAvailable { id: Uuid },

    #[snafu(
        display("VT engine error for TUI {id}: {message}"),
        visibility(pub(crate))
    )]
    VtEngine { id: Uuid, message: String },

    #[snafu(display("Invalid row range: {message}"))]
    InvalidRowRange { message: String },

    #[snafu(display("Row index {index} out of bounds (total lines: {total_lines})"))]
    RowIndexOutOfBounds { index: usize, total_lines: usize },

    #[snafu(display("Invalid column range: {message}"))]
    InvalidColRange { message: String },

    #[snafu(display("Column index {index} out of bounds (line length: {line_len})"))]
    ColIndexOutOfBounds { index: usize, line_len: usize },

    #[snafu(display("failed to build {rows}x{cols} styled-cell array: {source}"))]
    ArrayConversion {
        rows: usize,
        cols: usize,
        source: ndarray::ShapeError,
    },

    /// Wraps a dashboard failure surfaced by `tui-dashboard-core` (TCP bind,
    /// Loro encode) into this crate's error. Only constructed under the
    /// `dashboard` feature, when [`crate::serve`] starts the server.
    #[snafu(display("dashboard error: {message}"), visibility(pub(crate)))]
    Dashboard { message: String },

    /// Collapses the producer's foreign-boundary failures (directory creation,
    /// unix socket bind) into one observable message. Only constructed under the
    /// `publish` feature.
    #[cfg(feature = "publish")]
    #[snafu(display("publish error: {message}"), visibility(pub(crate)))]
    Publish { message: String },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(feature = "pyo3")]
impl From<Error> for pyo3::PyErr {
    fn from(err: Error) -> Self {
        use pyo3::exceptions::{PyIOError, PyKeyError, PyRuntimeError, PyValueError};
        let msg = err.to_string();
        match err {
            Error::TuiNotFound { .. } | Error::NoOutputAvailable { .. } => PyKeyError::new_err(msg),
            Error::InvalidRowRange { .. }
            | Error::InvalidColRange { .. }
            | Error::RowIndexOutOfBounds { .. }
            | Error::ColIndexOutOfBounds { .. } => PyValueError::new_err(msg),
            Error::ProcessSpawn { .. }
            | Error::WriteToTui { .. }
            | Error::ReadFromTui { .. }
            | Error::SignalTui { .. }
            | Error::ResizeTui { .. } => PyIOError::new_err(msg),
            Error::ArrayConversion { .. }
            | Error::Dashboard { .. }
            | Error::VtEngine { .. } => PyRuntimeError::new_err(msg),
            #[cfg(feature = "publish")]
            Error::Publish { .. } => PyRuntimeError::new_err(msg),
        }
    }
}
