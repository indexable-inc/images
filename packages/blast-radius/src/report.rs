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
            // A single-check cause is the same node twice (the cause drv is the
            // check's own per-unit derivation, e.g. lint/lint or
            // oci-image-builder-clippy-0.1.0/rust-oci-image-builder-clippy). Draw
            // those as one node labeled with the check name and skip the arrow;
            // multi-check causes still fan out cause -> check.
            for (index, cause) in self.causes.iter().enumerate() {
                let label = if cause.checks.len() == 1 {
                    &cause.checks[0]
                } else {
                    &cause.name
                };
                let _ = writeln!(out, "  c{index}[\"{label}\"]");
            }
            for (index, cause) in self.causes.iter().enumerate() {
                if cause.checks.len() == 1 {
                    continue;
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    // Locks in the single-check collapse and keeps it in sync with the
    // workflow's jq renderer (validated against the same shape by
    // tools/blast-radius-test.sh against tools/blast-radius-fixtures/).
    #[test]
    fn single_check_cause_collapses_to_one_node() {
        let report = Report {
            base: "aaaaaaa".into(),
            head: "bbbbbbb".into(),
            total: 120,
            changed: vec![
                "mcp-serverTools".into(),
                "rust-test-search_core".into(),
                "image-base".into(),
                "lint".into(),
            ],
            added: vec!["mcp-evalSmoke".into()],
            removed: vec![],
            categories: vec![
                Category { name: "rust".into(), count: 2 },
                Category { name: "mcp".into(), count: 2 },
                Category { name: "image".into(), count: 1 },
                Category { name: "lint".into(), count: 1 },
            ],
            causes: vec![
                CauseJson {
                    name: "ix-rust-workspace".into(),
                    checks: vec!["mcp-serverTools".into(), "rust-test-search_core".into()],
                },
                CauseJson {
                    name: "image-base-layer".into(),
                    checks: vec!["image-base".into()],
                },
                CauseJson {
                    name: "ix-images-lint".into(),
                    checks: vec!["lint".into()],
                },
            ],
        };
        let md = report.to_markdown();
        // Multi-check cause keeps its drv label and draws arrows.
        assert!(md.contains("c0[\"ix-rust-workspace\"]"));
        assert!(md.contains("c0 --> k"));
        // Single-check causes drop the drv label and use the check name, with
        // no outgoing arrow.
        assert!(md.contains("c1[\"image-base\"]"));
        assert!(md.contains("c2[\"lint\"]"));
        assert!(!md.contains("c1 -->"));
        assert!(!md.contains("c2 -->"));
        assert!(!md.contains("image-base-layer"));
        assert!(!md.contains("ix-images-lint"));
    }
}
