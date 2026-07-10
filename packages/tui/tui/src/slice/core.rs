use crate::{Error, Result};

/// A resolved 1-indexed inclusive range, with optional endpoints filled in from
/// the available extent.
#[derive(Debug, Clone, Copy)]
pub(super) struct Bounds {
    /// First selected index (1-indexed, inclusive).
    from: usize,
    /// Last selected index (1-indexed, inclusive).
    to: usize,
}

macro_rules! define_range {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy)]
        pub struct $name {
            pub from: Option<usize>,
            pub to: Option<usize>,
        }

        impl $name {
            #[must_use]
            pub const fn new(from: Option<usize>, to: Option<usize>) -> Self {
                Self { from, to }
            }

            pub(super) fn resolve(&self, extent: usize) -> Bounds {
                Bounds {
                    from: self.from.unwrap_or(1),
                    to: self.to.unwrap_or(extent),
                }
            }
        }
    };
}

define_range!(RowRange);
define_range!(ColRange);

pub fn slice_2d(lines: &[String], row_range: RowRange, col_range: ColRange) -> Result<Vec<String>> {
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    let total_lines = lines.len();
    let rows = row_range.resolve(total_lines);

    validate_row_range(rows.from, rows.to, total_lines)?;

    #[allow(clippy::indexing_slicing, reason = "row range validated above")]
    let selected_lines = &lines[rows.from - 1..rows.to];

    let result: Result<Vec<String>> = selected_lines
        .iter()
        .map(|line| {
            let char_count = line.chars().count();
            if char_count == 0 {
                return Ok(String::new());
            }

            let cols = col_range.resolve(char_count);
            validate_col_range(cols.from, cols.to, char_count)?;

            let chars: Vec<char> = line.chars().collect();
            #[allow(clippy::indexing_slicing, reason = "col range validated above")]
            let sliced: String = chars[cols.from - 1..cols.to].iter().collect();
            Ok(sliced)
        })
        .collect();

    result
}

fn validate_row_range(from: usize, to: usize, total_lines: usize) -> Result<()> {
    validate_range(from, to, RangeAxis::Row { total_lines })
}

fn validate_col_range(from: usize, to: usize, line_len: usize) -> Result<()> {
    validate_range(from, to, RangeAxis::Col { line_len })
}

#[derive(Clone, Copy)]
enum RangeAxis {
    Row { total_lines: usize },
    Col { line_len: usize },
}

impl RangeAxis {
    const fn name(self) -> &'static str {
        match self {
            Self::Row { .. } => "row",
            Self::Col { .. } => "col",
        }
    }

    const fn length(self) -> usize {
        match self {
            Self::Row { total_lines } => total_lines,
            Self::Col { line_len } => line_len,
        }
    }

    fn invalid(self, message: String) -> Error {
        match self {
            Self::Row { .. } => Error::InvalidRowRange { message },
            Self::Col { .. } => Error::InvalidColRange { message },
        }
    }

    const fn out_of_bounds(self, index: usize) -> Error {
        match self {
            Self::Row { total_lines } => Error::RowIndexOutOfBounds { index, total_lines },
            Self::Col { line_len } => Error::ColIndexOutOfBounds { index, line_len },
        }
    }
}

fn validate_range(from: usize, to: usize, axis: RangeAxis) -> Result<()> {
    let name = axis.name();
    if from == 0 {
        return Err(axis.invalid(format!("{name}-from must be >= 1 (1-indexed)")));
    }

    if to == 0 {
        return Err(axis.invalid(format!("{name}-to must be >= 1 (1-indexed)")));
    }

    if from > to {
        return Err(axis.invalid(format!("{name}-from ({from}) must be <= {name}-to ({to})")));
    }

    if from > axis.length() {
        return Err(axis.out_of_bounds(from));
    }

    if to > axis.length() {
        return Err(axis.out_of_bounds(to));
    }

    Ok(())
}
