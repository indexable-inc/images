//! One error type for the whole crate.

use std::path::PathBuf;

use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    #[snafu(display("run rust-analyzer scip (is it on PATH?)"))]
    RunRustAnalyzer { source: std::io::Error },

    #[snafu(display("rust-analyzer scip exited with {code}"))]
    RustAnalyzerFailed { code: String },

    #[snafu(display("read SCIP index {}", path.display()))]
    ReadIndex {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("read Soufflé program {}", path.display()))]
    ReadProgram {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("parse SCIP index"))]
    ParseIndex { source: protobuf::Error },

    #[snafu(display("read source {} for byte offsets", path.display()))]
    ReadSource {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("convert a SCIP position to a byte offset"))]
    Offset { source: std::num::TryFromIntError },

    #[snafu(display("write facts {}", path.display()))]
    WriteFacts {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("create scratch directory"))]
    Scratch { source: std::io::Error },

    #[snafu(display("run souffle (is it on PATH?)"))]
    RunSouffle { source: std::io::Error },

    #[snafu(display("souffle exited with {code}:\n{stderr}"))]
    SouffleFailed { code: String, stderr: String },

    #[snafu(display("read souffle output {}", path.display()))]
    ReadOutput {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display(
        "edit relation row {row}: column `{column}` is not a valid {expected}: {value:?}"
    ))]
    BadEditRow {
        row: usize,
        column: String,
        expected: String,
        value: String,
    },

    #[snafu(display("edit targets {path}, which is not a document in the index"))]
    EditUnknownPath { path: String },

    #[snafu(display("rename selector is empty (it would match every symbol)"))]
    EmptySelector,

    #[snafu(transparent)]
    Overlap { source: edit_applier::OverlapError },

    #[snafu(display("write rewritten file {}", path.display()))]
    WriteRewrite {
        path: PathBuf,
        source: std::io::Error,
    },
}
