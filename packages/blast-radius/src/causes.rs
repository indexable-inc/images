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

/// One node in a derivation graph: its human-readable name plus the `.drv`
/// basenames of its direct input derivations.
#[derive(Debug, Clone)]
pub struct DrvNode {
    pub name: String,
    pub inputs: Vec<String>,
}

/// A derivation graph keyed by `.drv` basename (`<hash>-<name>.drv`). The
/// basename is input-addressed, so an identical basename means an identical
/// derivation: a head node is unchanged iff its basename also exists at base.
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

/// Whether a head derivation differs from the base. Basenames are
/// input-addressed, so a head node is unchanged exactly when its basename also
/// appears in the base closure; a basename absent from base (or an unknown
/// node) counts as changed.
fn is_changed(base: &Graph, basename: &str) -> bool {
    !base.contains_key(basename)
}

/// Walk the changed sub-DAG reachable from `start` and collect the names of the
/// frontier derivations: changed nodes whose own derivation inputs are all
/// unchanged.
///
/// Unchanged nodes are pruned (their whole subtree is identical to the base by
/// definition), and descent stops at each frontier node (nothing changed lives
/// below it), so the traversal is bounded by the change, not the full closure.
///
/// Only `.drv` inputs are followed, not bare source inputs (`inputs.srcs`). A
/// changed source is part of the consuming derivation's input-addressed hash, so
/// that derivation's basename already moves and becomes the frontier: a crate
/// source edit lands on the crate's unit derivation (e.g. `mynoise-0.1.0`), the
/// readable cause, rather than a raw `cargo-unit-source-*` path. A check that
/// embeds a source directly with no wrapping derivation (e.g. a `runCommand`
/// over a filtered tree) is its own frontier, which is the right attribution.
fn collect_frontier(
    base: &Graph,
    head: &Graph,
    start: &str,
    seen: &mut BTreeSet<String>,
    out: &mut BTreeSet<String>,
) {
    if !seen.insert(start.to_owned()) {
        return;
    }
    if !is_changed(base, start) {
        return;
    }
    let Some(node) = head.get(start) else {
        return;
    };
    let changed_inputs: Vec<&String> = node
        .inputs
        .iter()
        .filter(|input| is_changed(base, input))
        .collect();
    if changed_inputs.is_empty() {
        out.insert(node.name.clone());
    } else {
        for input in changed_inputs {
            collect_frontier(base, head, input, seen, out);
        }
    }
}

/// Attribute each rebuilt check to its changed frontier derivations, then rank
/// the causes by fan-out (how many checks each rebuilds) and apply the graph
/// caps. `changed_checks` maps a check attribute name to its head `.drv`
/// basename.
pub fn root_causes(
    base: &Graph,
    head: &Graph,
    changed_checks: &BTreeMap<String, String>,
    caps: Caps,
) -> Vec<Cause> {
    let mut acc: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (attr, head_basename) in changed_checks {
        let mut frontier = BTreeSet::new();
        let mut seen = BTreeSet::new();
        collect_frontier(base, head, head_basename, &mut seen, &mut frontier);
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
    // intermediate unit, not the one real source edit. Basenames carry the
    // change: an unchanged input keeps its basename, a changed one moves it.
    #[test]
    fn single_source_edit_is_the_one_cause() {
        let base: Graph = [
            ("hh-glibc.drv".into(), node("glibc", &[])),
            ("h0-tui-source.drv".into(), node("tui-source", &[])),
            (
                "h0-rust-a.drv".into(),
                node("rust-a", &["h0-tui-source.drv", "hh-glibc.drv"]),
            ),
            (
                "h0-rust-b.drv".into(),
                node("rust-b", &["h0-tui-source.drv", "hh-glibc.drv"]),
            ),
        ]
        .into();
        // head: glibc basename identical, tui-source moved, so both checks moved.
        let head: Graph = [
            ("hh-glibc.drv".into(), node("glibc", &[])),
            ("h1-tui-source.drv".into(), node("tui-source", &[])),
            (
                "h1-rust-a.drv".into(),
                node("rust-a", &["h1-tui-source.drv", "hh-glibc.drv"]),
            ),
            (
                "h1-rust-b.drv".into(),
                node("rust-b", &["h1-tui-source.drv", "hh-glibc.drv"]),
            ),
        ]
        .into();
        let changed: BTreeMap<String, String> = [
            ("rust-a".into(), "h1-rust-a.drv".into()),
            ("rust-b".into(), "h1-rust-b.drv".into()),
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
            ("h0-a-source.drv".into(), node("a-source", &[])),
            ("h0-b-source.drv".into(), node("b-source", &[])),
            ("h0-rust-a.drv".into(), node("rust-a", &["h0-a-source.drv"])),
            ("h0-rust-b.drv".into(), node("rust-b", &["h0-b-source.drv"])),
        ]
        .into();
        let head: Graph = [
            ("h1-a-source.drv".into(), node("a-source", &[])),
            ("h1-b-source.drv".into(), node("b-source", &[])),
            ("h1-rust-a.drv".into(), node("rust-a", &["h1-a-source.drv"])),
            ("h1-rust-b.drv".into(), node("rust-b", &["h1-b-source.drv"])),
        ]
        .into();
        let changed: BTreeMap<String, String> = [
            ("rust-a".into(), "h1-rust-a.drv".into()),
            ("rust-b".into(), "h1-rust-b.drv".into()),
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
            ("hh-dep.drv".into(), node("dep", &[])),
            ("h0-rust-c.drv".into(), node("rust-c", &["hh-dep.drv"])),
        ]
        .into();
        let head: Graph = [
            ("hh-dep.drv".into(), node("dep", &[])),
            ("h1-rust-c.drv".into(), node("rust-c", &["hh-dep.drv"])),
        ]
        .into();
        let changed: BTreeMap<String, String> = [("rust-c".into(), "h1-rust-c.drv".into())].into();

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
        base.insert("h0-wide.drv".into(), node("wide", &[]));
        head.insert("h1-wide.drv".into(), node("wide", &[]));
        base.insert("h0-narrow.drv".into(), node("narrow", &[]));
        head.insert("h1-narrow.drv".into(), node("narrow", &[]));
        for check in ["rust-1", "rust-2", "rust-3"] {
            base.insert(format!("h0-{check}.drv"), node(check, &["h0-wide.drv"]));
            let hp = format!("h1-{check}.drv");
            head.insert(hp.clone(), node(check, &["h1-wide.drv"]));
            changed.insert(check.to_owned(), hp);
        }
        base.insert(
            "h0-rust-solo.drv".into(),
            node("rust-solo", &["h0-narrow.drv"]),
        );
        head.insert(
            "h1-rust-solo.drv".into(),
            node("rust-solo", &["h1-narrow.drv"]),
        );
        changed.insert("rust-solo".into(), "h1-rust-solo.drv".into());

        let causes = root_causes(&base, &head, &changed, caps);
        assert_eq!(causes.len(), 1);
        assert_eq!(causes[0].name, "wide");
        assert_eq!(causes[0].checks, vec!["rust-1", "rust-2"]);
    }
}
