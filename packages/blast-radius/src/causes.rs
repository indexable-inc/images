//! Root-cause attribution for the blast-radius report.
//!
//! Given the derivation graphs of the rebuilt checks at the base and head
//! revisions, find the *frontier* of change: derivations that differ between
//! base and head but whose own inputs are all unchanged. Those are the genuine
//! root causes (an edited crate source, a bumped dependency, a new toolchain),
//! as opposed to the noisy intermediate derivations whose hashes merely
//! propagate the change upward.
//!
//! The old nushell tool blamed every *direct* input of a rebuilt check whose
//! hash moved. Under per-unit Cargo builds a check's direct inputs are dozens of
//! per-crate unit derivations, so any broad change moved all their hashes at
//! once and every changed crate was credited as a cause of every check it sat
//! near. Walking down to the changed frontier collapses that hairball to the
//! handful of inputs a human actually changed.

use std::collections::{BTreeMap, BTreeSet};

/// One node in a derivation graph: its human-readable name plus the store paths
/// of its direct input derivations.
#[derive(Debug, Clone)]
pub struct DrvNode {
    pub name: String,
    pub inputs: Vec<String>,
}

/// A derivation graph keyed by `.drv` store path.
pub type Graph = BTreeMap<String, DrvNode>;

/// A root cause and the rebuilt checks it explains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cause {
    pub name: String,
    pub checks: Vec<String>,
}

/// The graph budget for the rendered flowchart. Only the highest fan-out causes,
/// and a few checks per cause, are drawn; the comment still lists every changed
/// check in full elsewhere.
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub max_causes: usize,
    pub max_checks_per_cause: usize,
}

/// The set of base store paths that exist for each derivation name. A head path
/// is "unchanged" iff a base derivation of the same name has that exact path.
fn base_paths_by_name(base: &Graph) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut by_name: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (path, node) in base {
        by_name
            .entry(node.name.as_str())
            .or_default()
            .insert(path.as_str());
    }
    by_name
}

/// Whether a head derivation path differs from the base. A path is unchanged
/// only when a base derivation of the same name carries that identical path;
/// anything else (new path, unknown node) counts as changed.
fn is_changed(head: &Graph, base_by_name: &BTreeMap<&str, BTreeSet<&str>>, path: &str) -> bool {
    match head.get(path) {
        Some(node) => !base_by_name
            .get(node.name.as_str())
            .is_some_and(|paths| paths.contains(path)),
        None => true,
    }
}

/// Walk the changed sub-DAG reachable from `start` and collect the names of the
/// frontier derivations: changed nodes whose own inputs are all unchanged.
///
/// Unchanged nodes are pruned (their whole subtree is identical to the base by
/// definition), and descent stops at each frontier node (nothing changed lives
/// below it), so the traversal is bounded by the change, not the full closure.
fn collect_frontier(
    head: &Graph,
    base_by_name: &BTreeMap<&str, BTreeSet<&str>>,
    start: &str,
    seen: &mut BTreeSet<String>,
    out: &mut BTreeSet<String>,
) {
    if !seen.insert(start.to_owned()) {
        return;
    }
    if !is_changed(head, base_by_name, start) {
        return;
    }
    let Some(node) = head.get(start) else {
        return;
    };
    let changed_inputs: Vec<&String> = node
        .inputs
        .iter()
        .filter(|input| is_changed(head, base_by_name, input))
        .collect();
    if changed_inputs.is_empty() {
        out.insert(node.name.clone());
    } else {
        for input in changed_inputs {
            collect_frontier(head, base_by_name, input, seen, out);
        }
    }
}

/// Attribute each rebuilt check to its changed frontier derivations, then rank
/// the causes by fan-out (how many checks each rebuilds) and apply the graph
/// caps. `changed_checks` maps a check attribute name to its head `.drv` path.
pub fn root_causes(
    base: &Graph,
    head: &Graph,
    changed_checks: &BTreeMap<String, String>,
    caps: Caps,
) -> Vec<Cause> {
    let base_by_name = base_paths_by_name(base);
    let mut acc: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (attr, head_path) in changed_checks {
        let mut frontier = BTreeSet::new();
        let mut seen = BTreeSet::new();
        collect_frontier(head, &base_by_name, head_path, &mut seen, &mut frontier);
        for cause in frontier {
            acc.entry(cause).or_default().insert(attr.clone());
        }
    }

    let mut causes: Vec<Cause> = acc
        .into_iter()
        .map(|(name, checks)| Cause {
            name,
            checks: checks.into_iter().collect(),
        })
        .collect();
    // Highest fan-out first; ties broken by name so the output is deterministic.
    causes.sort_by(|left, right| {
        right
            .checks
            .len()
            .cmp(&left.checks.len())
            .then_with(|| left.name.cmp(&right.name))
    });
    causes.truncate(caps.max_causes);
    for cause in &mut causes {
        cause.checks.truncate(caps.max_checks_per_cause);
    }
    causes
}

/// A check's category for the v1 breakdown: the segment before the first dash
/// (`image-foo` -> `image`, `rust-test-bar` -> `rust`, `lint` -> `lint`).
pub fn category(name: &str) -> &str {
    match name.split('-').next() {
        Some(head) if !head.is_empty() => head,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, inputs: &[&str]) -> DrvNode {
        DrvNode {
            name: name.to_owned(),
            inputs: inputs.iter().map(|input| (*input).to_owned()).collect(),
        }
    }

    const CAPS: Caps = Caps {
        max_causes: 6,
        max_checks_per_cause: 5,
    };

    // A single edited crate source fans out to both checks that embed it, while
    // the unchanged glibc below it is never blamed. This is the case the old
    // direct-reference heuristic got wrong: it would have credited every moved
    // intermediate unit, not the one real source edit.
    #[test]
    fn single_source_edit_is_the_one_cause() {
        let base: Graph = [
            ("/b/glibc.drv".into(), node("glibc", &[])),
            ("/b/src.drv".into(), node("tui-source", &[])),
            (
                "/b/a.drv".into(),
                node("rust-a", &["/b/src.drv", "/b/glibc.drv"]),
            ),
            (
                "/b/b.drv".into(),
                node("rust-b", &["/b/src.drv", "/b/glibc.drv"]),
            ),
        ]
        .into();
        // head: glibc identical, tui-source moved, so both checks moved too.
        let head: Graph = [
            ("/b/glibc.drv".into(), node("glibc", &[])),
            ("/h/src.drv".into(), node("tui-source", &[])),
            (
                "/h/a.drv".into(),
                node("rust-a", &["/h/src.drv", "/b/glibc.drv"]),
            ),
            (
                "/h/b.drv".into(),
                node("rust-b", &["/h/src.drv", "/b/glibc.drv"]),
            ),
        ]
        .into();
        let changed: BTreeMap<String, String> = [
            ("rust-a".into(), "/h/a.drv".into()),
            ("rust-b".into(), "/h/b.drv".into()),
        ]
        .into();

        let causes = root_causes(&base, &head, &changed, CAPS);
        assert_eq!(
            causes,
            vec![Cause {
                name: "tui-source".into(),
                checks: vec!["rust-a".into(), "rust-b".into()],
            }]
        );
    }

    // Two independent source edits each fan out only to their own check: no
    // cross-blame between unrelated crates (the hairball the rewrite kills).
    #[test]
    fn independent_edits_do_not_cross_blame() {
        let base: Graph = [
            ("/b/sa.drv".into(), node("a-source", &[])),
            ("/b/sb.drv".into(), node("b-source", &[])),
            ("/b/a.drv".into(), node("rust-a", &["/b/sa.drv"])),
            ("/b/b.drv".into(), node("rust-b", &["/b/sb.drv"])),
        ]
        .into();
        let head: Graph = [
            ("/h/sa.drv".into(), node("a-source", &[])),
            ("/h/sb.drv".into(), node("b-source", &[])),
            ("/h/a.drv".into(), node("rust-a", &["/h/sa.drv"])),
            ("/h/b.drv".into(), node("rust-b", &["/h/sb.drv"])),
        ]
        .into();
        let changed: BTreeMap<String, String> = [
            ("rust-a".into(), "/h/a.drv".into()),
            ("rust-b".into(), "/h/b.drv".into()),
        ]
        .into();

        let causes = root_causes(&base, &head, &changed, CAPS);
        assert_eq!(
            causes,
            vec![
                Cause {
                    name: "a-source".into(),
                    checks: vec!["rust-a".into()],
                },
                Cause {
                    name: "b-source".into(),
                    checks: vec!["rust-b".into()],
                },
            ]
        );
    }

    // A check whose own derivation changed while all its inputs stayed put is
    // its own root cause (e.g. the check definition was edited).
    #[test]
    fn check_with_only_self_changed_is_its_own_cause() {
        let base: Graph = [
            ("/b/dep.drv".into(), node("dep", &[])),
            ("/b/c.drv".into(), node("rust-c", &["/b/dep.drv"])),
        ]
        .into();
        let head: Graph = [
            ("/b/dep.drv".into(), node("dep", &[])),
            ("/h/c.drv".into(), node("rust-c", &["/b/dep.drv"])),
        ]
        .into();
        let changed: BTreeMap<String, String> = [("rust-c".into(), "/h/c.drv".into())].into();

        let causes = root_causes(&base, &head, &changed, CAPS);
        assert_eq!(
            causes,
            vec![Cause {
                name: "rust-c".into(),
                checks: vec!["rust-c".into()],
            }]
        );
    }

    // Causes rank by fan-out, and the per-cause check list is capped.
    #[test]
    fn causes_rank_by_fanout_and_cap() {
        let caps = Caps {
            max_causes: 1,
            max_checks_per_cause: 2,
        };
        let mut base = Graph::new();
        let mut head = Graph::new();
        let mut changed = BTreeMap::new();
        // `wide` feeds three checks; `narrow` feeds one. Only `wide` survives the
        // max_causes=1 cap, and its check list is truncated to two.
        base.insert("/b/wide.drv".into(), node("wide", &[]));
        head.insert("/h/wide.drv".into(), node("wide", &[]));
        base.insert("/b/narrow.drv".into(), node("narrow", &[]));
        head.insert("/h/narrow.drv".into(), node("narrow", &[]));
        for check in ["rust-1", "rust-2", "rust-3"] {
            let bp = format!("/b/{check}.drv");
            let hp = format!("/h/{check}.drv");
            base.insert(bp, node(check, &["/b/wide.drv"]));
            head.insert(hp.clone(), node(check, &["/h/wide.drv"]));
            changed.insert(check.to_owned(), hp);
        }
        base.insert("/b/solo.drv".into(), node("rust-solo", &["/b/narrow.drv"]));
        head.insert("/h/solo.drv".into(), node("rust-solo", &["/h/narrow.drv"]));
        changed.insert("rust-solo".into(), "/h/solo.drv".into());

        let causes = root_causes(&base, &head, &changed, caps);
        assert_eq!(causes.len(), 1);
        assert_eq!(causes[0].name, "wide");
        assert_eq!(causes[0].checks, vec!["rust-1", "rust-2"]);
    }

    #[test]
    fn category_splits_on_first_dash() {
        assert_eq!(category("image-foo"), "image");
        assert_eq!(category("rust-test-bar"), "rust");
        assert_eq!(category("lint"), "lint");
    }
}
