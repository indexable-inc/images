//! Plain table output for pipes, scripts, and `--plain`.

use std::fmt::Write as _;
use std::io::Write;

use color_eyre::eyre::Result;

use crate::model::{PatchRow, PrRef, PrStatus};

const HEADERS: [&str; 8] = [
    "FORK",
    "PATCH",
    "INTENT",
    "PR",
    "STATE",
    "CI",
    "REVIEW",
    "UNRESOLVED",
];

pub fn unresolved_cell(status: &PrStatus) -> String {
    let PrStatus {
        unresolved,
        unresolved_truncated,
        ..
    } = status;
    let suffix = if *unresolved_truncated { "+" } else { "" };
    format!("{unresolved}{suffix}")
}

pub fn cells(row: &PatchRow) -> [String; 8] {
    let dash = || "-".to_owned();
    let status = row.status.as_ref();
    [
        row.fork.clone(),
        row.file.clone(),
        row.intent.clone().unwrap_or_else(dash),
        row.pr.as_ref().map_or_else(dash, PrRef::short),
        status.map_or_else(dash, |status| status.state.label().to_owned()),
        status
            .and_then(|status| status.ci)
            .map_or_else(dash, |ci| ci.label().to_owned()),
        status
            .and_then(|status| status.review)
            .map_or_else(dash, |review| review.label().to_owned()),
        status.map_or_else(dash, unresolved_cell),
    ]
}

/// Print all rows as one aligned table.
pub fn print(rows: &[PatchRow], out: &mut impl Write) -> Result<()> {
    let table: Vec<[String; 8]> = rows.iter().map(cells).collect();
    let mut widths: [usize; 8] = HEADERS.map(str::len);
    for row in &table {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.len());
        }
    }
    let print_row = |out: &mut dyn Write, cells: &[String; 8]| -> std::io::Result<()> {
        let mut line = String::new();
        for (width, cell) in widths.iter().zip(cells) {
            if !line.is_empty() {
                line.push_str("  ");
            }
            let _ = write!(line, "{cell:<width$}");
        }
        writeln!(out, "{}", line.trim_end())
    };
    print_row(out, &HEADERS.map(str::to_owned))?;
    for row in &table {
        print_row(out, row)?;
    }
    Ok(())
}
