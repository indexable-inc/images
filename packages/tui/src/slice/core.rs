use crate::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct RowRange {
    pub from: Option<usize>,
    pub to: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct ColRange {
    pub from: Option<usize>,
    pub to: Option<usize>,
}

impl RowRange {
    #[must_use]
    pub const fn new(from: Option<usize>, to: Option<usize>) -> Self {
        Self { from, to }
    }

    pub(super) fn resolve(&self, total_lines: usize) -> (usize, usize) {
        let from = self.from.unwrap_or(1);
        let to = self.to.unwrap_or(total_lines);
        (from, to)
    }
}

impl ColRange {
    #[must_use]
    pub const fn new(from: Option<usize>, to: Option<usize>) -> Self {
        Self { from, to }
    }

    pub(super) fn resolve(&self, line_len: usize) -> (usize, usize) {
        let from = self.from.unwrap_or(1);
        let to = self.to.unwrap_or(line_len);
        (from, to)
    }
}

pub fn slice_2d(lines: &[String], row_range: RowRange, col_range: ColRange) -> Result<Vec<String>> {
    if lines.is_empty() {
        return Ok(Vec::new());
    }

    let total_lines = lines.len();
    let (row_from, row_to) = row_range.resolve(total_lines);

    validate_row_range(row_from, row_to, total_lines)?;

    #[allow(clippy::indexing_slicing, reason = "row range validated above")]
    let selected_lines = &lines[row_from - 1..row_to];

    let result: Result<Vec<String>> = selected_lines
        .iter()
        .map(|line| {
            let char_count = line.chars().count();
            if char_count == 0 {
                return Ok(String::new());
            }

            let (col_from, col_to) = col_range.resolve(char_count);
            validate_col_range(col_from, col_to, char_count)?;

            let chars: Vec<char> = line.chars().collect();
            #[allow(clippy::indexing_slicing, reason = "col range validated above")]
            let sliced: String = chars[col_from - 1..col_to].iter().collect();
            Ok(sliced)
        })
        .collect();

    result
}

fn validate_row_range(from: usize, to: usize, total_lines: usize) -> Result<()> {
    if from == 0 {
        return Err(Error::InvalidRowRange {
            message: "row-from must be >= 1 (1-indexed)".into(),
        });
    }

    if to == 0 {
        return Err(Error::InvalidRowRange {
            message: "row-to must be >= 1 (1-indexed)".into(),
        });
    }

    if from > to {
        return Err(Error::InvalidRowRange {
            message: format!("row-from ({from}) must be <= row-to ({to})"),
        });
    }

    if from > total_lines {
        return Err(Error::RowIndexOutOfBounds {
            index: from,
            total_lines,
        });
    }

    if to > total_lines {
        return Err(Error::RowIndexOutOfBounds {
            index: to,
            total_lines,
        });
    }

    Ok(())
}

fn validate_col_range(from: usize, to: usize, line_len: usize) -> Result<()> {
    if from == 0 {
        return Err(Error::InvalidColRange {
            message: "col-from must be >= 1 (1-indexed)".into(),
        });
    }

    if to == 0 {
        return Err(Error::InvalidColRange {
            message: "col-to must be >= 1 (1-indexed)".into(),
        });
    }

    if from > to {
        return Err(Error::InvalidColRange {
            message: format!("col-from ({from}) must be <= col-to ({to})"),
        });
    }

    if from > line_len {
        return Err(Error::ColIndexOutOfBounds {
            index: from,
            line_len,
        });
    }

    if to > line_len {
        return Err(Error::ColIndexOutOfBounds {
            index: to,
            line_len,
        });
    }

    Ok(())
}
