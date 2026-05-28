#![allow(
    clippy::missing_errors_doc,
    reason = "library API errors are documented via the typed `Error` enum"
)]
#![allow(
    clippy::significant_drop_tightening,
    reason = "guard-then-extract is the natural read pattern for the VT100 parser"
)]
#![allow(
    clippy::struct_excessive_bools,
    reason = "VT100 cell attributes are intrinsically four parallel booleans"
)]
#![allow(
    clippy::option_if_let_else,
    reason = "explicit match is clearer than map_or for cell extraction sites"
)]

mod actor;
mod cache;
mod error;
mod manager;
mod slice;
mod types;

pub use cache::Cache;
pub use error::{Error, Result};
pub use manager::TuiManager;
pub use manager::reader::FullOutput;
pub use slice::{ColRange, RowRange, slice_2d};
pub use types::{StyledCell, TuiInstance};

#[cfg(test)]
mod tests;
