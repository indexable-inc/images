//! Shared rendering for the two read-only registry reports.
//!
//! [`drift`](crate::drift) and [`pin`](crate::pin) answer different questions
//! about the same fork registry and present the answer the same three ways: JSON
//! for machines, a markdown table for step summaries and PR bodies, and that
//! table under a heading for a person. The table's bytes are pinned to nu's
//! `to md --pretty` because the fork-sync workflow embeds it in both a step
//! summary and a PR body, so there is one implementation of it here rather than
//! one per report drifting away from the other.

use anstream::println;
use color_eyre::eyre::{Result, eyre};
use serde::Serialize;

use crate::style::{CYAN, paint};

/// Refuse the two output flags together, before any forge read happens.
///
/// # Errors
/// Fails when both `--json` and `--markdown` were passed.
pub(crate) fn check_flags(lane: &str, json: bool, markdown: bool) -> Result<()> {
    if json && markdown {
        return Err(eyre!(
            "upstream-sync: {lane}: --json and --markdown are mutually exclusive"
        ));
    }
    Ok(())
}

/// Print the report in whichever of the three shapes was asked for.
///
/// # Errors
/// Fails when the rows cannot be serialised to JSON.
pub(crate) fn emit<T: Serialize>(
    rows: &[T],
    table: &str,
    heading: &str,
    json: bool,
    markdown: bool,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(rows)?);
    } else if markdown {
        println!("{table}");
    } else {
        println!("{}", paint(CYAN, heading));
        println!("{table}");
    }
    Ok(())
}

/// A GitHub-flavored markdown table, byte-compatible with nu's
/// `to md --pretty`: every column padded to the widest of its cells and its
/// header, and a dash rule of the same widths.
///
/// # Panics
/// Panics when a row is shorter than `headers`, which is a programming error in
/// the caller rather than anything a run can produce: the cells are built
/// alongside the header list in the same function.
pub(crate) fn markdown_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            rows.iter()
                .map(|row| row[i].len())
                .chain([h.len()])
                .max()
                .unwrap_or_default()
        })
        .collect();
    let line = |vals: &[String]| {
        let padded: Vec<String> = vals
            .iter()
            .zip(&widths)
            .map(|(v, w)| format!("{v:<w$}"))
            .collect();
        format!("| {} |", padded.join(" | "))
    };

    let mut out = vec![
        line(&headers.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>()),
        line(
            &widths
                .iter()
                .map(|w| "-".repeat(*w))
                .collect::<Vec<String>>(),
        ),
    ];
    out.extend(rows.iter().map(|row| line(row.as_slice())));
    out.join("\n")
}
