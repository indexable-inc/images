//! Plan diffing: desired plan vs journal, with human-readable reasons.

use std::collections::BTreeMap;

use efx_ir::{Effect, EffectId, Plan, Value};
use snafu::ResultExt;

use crate::EngineError;
use crate::journal::{Journal, RunEffect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The effect's id has a successful journal record; nothing to do.
    Cached,
    /// The effect's id is new; it must run.
    Execute,
}

/// The per-effect outcome of a plan diff, in topological order.
#[derive(Clone, Debug)]
pub struct Decision {
    pub name: String,
    pub kind: String,
    pub id: EffectId,
    pub verdict: Verdict,
    pub reason: String,
    pub input_signatures: BTreeMap<String, String>,
}

/// A journal entry whose id no longer appears in the plan. Reported, never
/// destroyed: the journal is history, not a mirror of the plan.
#[derive(Clone, Debug)]
pub struct Orphan {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub recorded_at: u64,
}

#[derive(Debug)]
pub struct PlanReport {
    pub decisions: Vec<Decision>,
    pub orphans: Vec<Orphan>,
}

/// Diffs `plan` against `journal`. Pure: same plan + same journal always
/// yields the same report, in the same order.
///
/// # Errors
///
/// Returns [`EngineError::Plan`] when the plan's graph is invalid.
pub fn plan(plan: &Plan, journal: &Journal) -> Result<PlanReport, EngineError> {
    let ids = plan.effect_ids().context(crate::PlanSnafu)?;
    let previous: BTreeMap<&str, &RunEffect> = journal
        .state
        .runs
        .last()
        .map(|run| {
            run.effects
                .iter()
                .map(|effect| (effect.name.as_str(), effect))
                .collect()
        })
        .unwrap_or_default();

    let mut decisions = Vec::new();
    for effect in plan.topo_order().context(crate::PlanSnafu)? {
        let id = ids[&effect.name];
        let input_signatures = Plan::input_signatures(effect, &ids);
        let (verdict, reason) = if journal.is_cached(&id) {
            (Verdict::Cached, "unchanged".to_owned())
        } else {
            let reason = explain(
                effect,
                id,
                &input_signatures,
                previous.get(effect.name.as_str()),
            );
            (Verdict::Execute, reason)
        };
        decisions.push(Decision {
            name: effect.name.clone(),
            kind: effect.kind.clone(),
            id,
            verdict,
            reason,
            input_signatures,
        });
    }

    let current: Vec<String> = decisions.iter().map(|d| d.id.to_hex()).collect();
    let orphans = journal
        .state
        .entries
        .iter()
        .filter(|(id, _)| !current.contains(id))
        .map(|(id, entry)| Orphan {
            id: id.clone(),
            name: entry.name.clone(),
            kind: entry.kind.clone(),
            recorded_at: entry.recorded_at,
        })
        .collect();

    Ok(PlanReport { decisions, orphans })
}

/// Why does this effect need to execute? Compared against its namesake in
/// the previous run, signature by signature.
fn explain(
    effect: &Effect,
    id: EffectId,
    signatures: &BTreeMap<String, String>,
    previous: Option<&&RunEffect>,
) -> String {
    let Some(previous) = previous else {
        return "new effect".to_owned();
    };
    let mut causes = Vec::new();
    for (key, signature) in signatures {
        match previous.input_signatures.get(key) {
            None => causes.push(format!("input `{key}` added")),
            Some(old) if old != signature => {
                let cause = match effect.inputs.get(key) {
                    Some(Value::Ref(r)) => format!("upstream `{}` changed", r.effect),
                    _ => format!("input `{key}` changed"),
                };
                if !causes.contains(&cause) {
                    causes.push(cause);
                }
            }
            Some(_) => {}
        }
    }
    for key in previous.input_signatures.keys() {
        if !signatures.contains_key(key) {
            causes.push(format!("input `{key}` removed"));
        }
    }
    if causes.is_empty() {
        if previous.id == id {
            // Same identity but no successful record: the previous attempt
            // failed or was skipped.
            "previous run did not succeed".to_owned()
        } else {
            // Same inputs, different id: the kind or executor changed.
            "effect definition changed".to_owned()
        }
    } else {
        causes.join(", ")
    }
}
