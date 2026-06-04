//! The blast-radius report: the JSON the trusted workflow renders, plus a local
//! Markdown rendering for human `nix run .#blast-radius` invocations.
//!
//! The JSON schema is a contract with `.github/workflows/blast-radius.yml`,
//! whose trusted job validates this shape and rebuilds the comment from it. The
//! Markdown here mirrors that job's renderer (shared check nodes, capped) so a
//! local run shows what the posted comment will look like.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;

use crate::causes::{Cause, category};

#[derive(Debug, Serialize)]
pub struct Category {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize)]
pub struct CauseJson {
    pub name: String,
    pub checks: Vec<String>,
}

impl From<Cause> for CauseJson {
    fn from(cause: Cause) -> Self {
        Self {
            name: cause.name,
            checks: cause.checks,
        }
    }
}

/// The full report, serialized as the `report.json` the workflow consumes.
#[derive(Debug, Serialize)]
pub struct Report {
    pub base: String,
    pub head: String,
    pub total: usize,
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub categories: Vec<Category>,
    pub causes: Vec<CauseJson>,
}

/// Count `changed + added` checks by category, most-rebuilt family first.
pub fn categories(changed: &[String], added: &[String]) -> Vec<Category> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for name in changed.iter().chain(added) {
        *counts.entry(category(name)).or_insert(0) += 1;
    }
    let mut categories: Vec<Category> = counts
        .into_iter()
        .map(|(name, count)| Category {
            name: name.to_owned(),
            count,
        })
        .collect();
    categories.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.name.cmp(&right.name))
    });
    categories
}

impl Report {
    /// Render the sticky-comment Markdown, mirroring the trusted workflow
    /// renderer: one shared flowchart node per check, capped to the drawn causes.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        out.push_str("<!-- blast-radius -->\n### Blast radius\n\n");
        let rebuilt = self.changed.len() + self.added.len();
        let _ = writeln!(
            out,
            "`{rebuilt}` of `{total}` checks would rebuild between base `{base}` and head `{head}`.",
            total = self.total,
            base = self.base,
            head = self.head,
        );

        if !self.added.is_empty() || !self.removed.is_empty() {
            let _ = writeln!(
                out,
                "\n{} added, {} removed",
                self.added.len(),
                self.removed.len()
            );
        }

        if !self.categories.is_empty() {
            out.push_str("\n```mermaid\npie showData title Rebuilt checks by category\n");
            for category in &self.categories {
                let _ = writeln!(out, "  \"{}\" : {}", category.name, category.count);
            }
            out.push_str("```\n");
        }

        if !self.causes.is_empty() {
            // One node per check, shared across causes, so the flowchart stays
            // under Mermaid's node budget. Sorted+deduped to match the renderer.
            let mut checks: Vec<&str> = self
                .causes
                .iter()
                .flat_map(|cause| cause.checks.iter().map(String::as_str))
                .collect();
            checks.sort_unstable();
            checks.dedup();

            out.push_str("\n```mermaid\nflowchart LR\n");
            for (index, cause) in self.causes.iter().enumerate() {
                let _ = writeln!(out, "  c{index}[\"{}\"]", cause.name);
            }
            for (index, cause) in self.causes.iter().enumerate() {
                for check in &cause.checks {
                    let node = checks.iter().position(|name| name == check).unwrap_or(0);
                    let _ = writeln!(out, "  c{index} --> k{node}[\"{check}\"]");
                }
            }
            out.push_str("```\n");
        }

        if !self.changed.is_empty() {
            out.push_str("\n<details><summary>changed checks</summary>\n\n");
            for name in &self.changed {
                let _ = writeln!(out, "- {name}");
            }
            out.push_str("\n</details>\n");
        }

        out
    }
}
