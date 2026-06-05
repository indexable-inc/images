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

/// Maximum number of changed-check bullets to render in the `<details>` list.
/// A PR touching a shared input rebuilds thousands of checks (on ix, 3817 of
/// 4315 once), and the uncapped list produced a ~300 KB body that GitHub
/// rejected with HTTP 422 ("Body is too long", 65536-char limit), so no comment
/// posted at all. The trusted jq renderer in `.github/workflows/blast-radius.yml`
/// caps identically; the `<summary>` carries the true total and the full list
/// lives in the run artifact and logs.
pub const CHANGED_LIST_CAP: usize = 200;

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
    /// Per-attribute wall-clock seconds from the prior successful Check run on
    /// the base branch (see [`crate::timings`]). Empty when no prior run was
    /// available; missing attrs are checks that were a substituter hit (never
    /// rebuilt) or are new on this PR.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub timings: BTreeMap<String, f64>,
    /// Per-phase wall-clock seconds for this report's producer pipeline.
    /// Keys are stable kebab-case (see `main::record_phase`); values are raw
    /// seconds with no rounding so a downstream trend stays precise.
    #[serde(default, rename = "phaseTimings")]
    pub phase_timings: BTreeMap<String, f64>,
}

/// Format `seconds` as a short, scannable suffix: `<1s` under a second,
/// `12s` under a minute, `5m` under an hour, `2h` beyond. Rounding goes
/// through `f64::round` (round half-away-from-zero) so the output matches
/// the jq renderer in `.github/workflows/blast-radius.yml`, whose
/// `(x + 0.5) | floor` is round half-up and agrees for non-negative values.
/// `{:.0}` alone would use Rust's round-half-to-even and silently diverge.
pub fn format_seconds(seconds: f64) -> String {
    if seconds < 1.0 {
        "<1s".to_owned()
    } else if seconds < 60.0 {
        format!("{:.0}s", seconds.round())
    } else if seconds < 3600.0 {
        format!("{:.0}m", (seconds / 60.0).round())
    } else {
        format!("{:.0}h", (seconds / 3600.0).round())
    }
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
        // Suffix any check label with ` (12s)` when the base-branch timings
        // include it. A missing attr is a substituter hit or a new check on
        // this PR (no base timing) and renders bare. Shared by the cause
        // flowchart and the changed-checks list so both stay consistent.
        let label_for = |attr: &str| {
            self.timings.get(attr).map_or_else(
                || attr.to_owned(),
                |seconds| format!("{attr} ({})", format_seconds(*seconds)),
            )
        };

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
                    label_for(&cause.checks[0])
                } else {
                    cause.name.clone()
                };
                let _ = writeln!(out, "  c{index}[\"{label}\"]");
            }
            for (index, cause) in self.causes.iter().enumerate() {
                if cause.checks.len() == 1 {
                    continue;
                }
                for check in &cause.checks {
                    let node = checks.iter().position(|name| name == check).unwrap_or(0);
                    let label = label_for(check);
                    let _ = writeln!(out, "  c{index} --> k{node}[\"{label}\"]");
                }
            }
            out.push_str("```\n");
        }

        if !self.changed.is_empty() {
            let total = self.changed.len();
            let _ = writeln!(out, "\n<details><summary>changed checks ({total})</summary>\n");
            for name in self.changed.iter().take(CHANGED_LIST_CAP) {
                let _ = writeln!(out, "- {}", label_for(name));
            }
            if total > CHANGED_LIST_CAP {
                let _ = writeln!(
                    out,
                    "- ...and {} more (see the Blast radius check logs)",
                    total - CHANGED_LIST_CAP
                );
            }
            out.push_str("\n</details>\n");
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report(timings: BTreeMap<String, f64>) -> Report {
        Report {
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
            timings,
            phase_timings: BTreeMap::new(),
        }
    }

    // Locks in the single-check collapse and keeps it in sync with the
    // workflow's jq renderer (validated against the same shape by
    // tools/blast-radius-test.sh against tools/blast-radius-fixtures/).
    #[test]
    fn single_check_cause_collapses_to_one_node() {
        let md = sample_report(BTreeMap::new()).to_markdown();
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

    // When a base-branch Check artifact is fed in, every known attr is
    // suffixed with `(<duration>)` in the cause flowchart and the
    // changed-checks list; unknown attrs render bare.
    #[test]
    fn timings_annotate_checks_when_present() {
        let timings: BTreeMap<String, f64> = [
            ("mcp-serverTools".to_owned(), 42.0),
            ("rust-test-search_core".to_owned(), 130.0),
            ("image-base".to_owned(), 0.6),
            // `lint` deliberately omitted: cache hit or new check, no suffix.
        ]
        .into();
        let md = sample_report(timings).to_markdown();
        // Multi-check cause: per-check arrows carry the suffix. Check nodes
        // are k<index> into the sorted+deduped unique check list, so
        // mcp-serverTools is k2 and rust-test-search_core is k3.
        assert!(md.contains("k2[\"mcp-serverTools (42s)\"]"));
        assert!(md.contains("k3[\"rust-test-search_core (2m)\"]"));
        // Single-check cause: collapsed node carries the suffix.
        assert!(md.contains("c1[\"image-base (<1s)\"]"));
        // Known timing missing: bare label, no parens.
        assert!(md.contains("c2[\"lint\"]"));
        // Changed-checks list mirrors the same annotation.
        assert!(md.contains("- mcp-serverTools (42s)\n"));
        assert!(md.contains("- lint\n"));
    }

    // A PR that rebuilds thousands of checks must not blow GitHub's 65536-char
    // comment body limit: the list caps at CHANGED_LIST_CAP with an overflow
    // note, while the summary still carries the true total. Mirrors the trusted
    // jq renderer's cap (tools/blast-radius-test.sh asserts the same bound).
    #[test]
    fn caps_long_changed_list() {
        let mut report = sample_report(BTreeMap::new());
        report.changed = (0..4000).map(|i| format!("rust-test-crate-{i}")).collect();
        let md = report.to_markdown();
        assert!(md.contains("<summary>changed checks (4000)</summary>"));
        let bullets = md
            .lines()
            .filter(|line| line.starts_with("- rust-test-crate-"))
            .count();
        assert_eq!(bullets, CHANGED_LIST_CAP);
        assert!(md.contains(&format!("- ...and {} more", 4000 - CHANGED_LIST_CAP)));
        assert!(md.len() < 65_536);
    }

    #[test]
    fn format_seconds_buckets() {
        assert_eq!(format_seconds(0.4), "<1s");
        assert_eq!(format_seconds(12.4), "12s");
        assert_eq!(format_seconds(89.9), "1m");
        assert_eq!(format_seconds(7200.0), "2h");
    }
}
