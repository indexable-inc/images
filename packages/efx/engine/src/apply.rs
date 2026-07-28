//! Topological, level-parallel execution of a plan against a journal.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use efx_ir::{Effect, Literal, Plan, Value};
use snafu::ResultExt;

use crate::diff::{self, Decision, Orphan, Verdict};
use crate::journal::{Action, Journal, JournalEntry, RunEffect, RunRecord, Status, unix_now};
use crate::{EngineError, ExecuteError, ExecuteRequest, Executor, Outputs, Registry};

/// What one `apply` did, in topological order.
#[derive(Debug)]
pub struct RunReport {
    pub effects: Vec<RunEffect>,
    pub orphans: Vec<Orphan>,
}

impl RunReport {
    #[must_use]
    pub fn count(&self, action: Action) -> usize {
        self.effects.iter().filter(|e| e.action == action).count()
    }

    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.effects
            .iter()
            .all(|e| matches!(e.action, Action::Executed | Action::Cached))
    }
}

struct Job<'a> {
    effect: &'a Effect,
    executor: &'a dyn Executor,
    request: ExecuteRequest,
}

struct Execution {
    result: Result<Outputs, ExecuteError>,
    duration_ms: u128,
}

struct JobOutcome<'a> {
    effect: &'a Effect,
    execution: Execution,
}

#[derive(Default)]
struct RunState {
    outputs: BTreeMap<String, Outputs>,
    halted: BTreeSet<String>,
    results: BTreeMap<String, RunEffect>,
}

/// Executes `plan` against `journal`, using the executors in `registry`.
///
/// Cached effects are skipped; the rest run as soon as their dependency
/// level is complete, and effects within a level are independent so they run
/// in parallel. Successful results are recorded in `journal` (keyed by
/// effect id) together with a `RunRecord` for reporting, then saved. A
/// failing executor marks the effect failed and its transitive dependents
/// skipped; unrelated branches still run.
///
/// # Errors
///
/// Returns [`EngineError::Plan`] for an invalid graph,
/// [`EngineError::UnknownExecutor`] when an effect that must execute has no
/// registered executor, and journal errors from the final save.
pub fn apply(
    plan: &Plan,
    journal: &mut Journal,
    registry: &Registry,
) -> Result<RunReport, EngineError> {
    let report = diff::plan(plan, journal)?;
    let decisions: BTreeMap<&str, &Decision> = report
        .decisions
        .iter()
        .map(|d| (d.name.as_str(), d))
        .collect();
    let order = plan.topo_order().context(crate::PlanSnafu)?;

    // Executors are config, not data: verify them all before running anything.
    for effect in &order {
        if decisions[effect.name.as_str()].verdict == Verdict::Execute
            && registry.get(&effect.executor).is_none()
        {
            return Err(EngineError::UnknownExecutor {
                effect: effect.name.clone(),
                executor: effect.executor.clone(),
            });
        }
    }

    let mut state = RunState::default();
    for effects in levels(&order) {
        let jobs = stage_level(&effects, &decisions, journal, registry, &mut state);
        settle_level(run_level(jobs), journal, &mut state);
    }

    let effects: Vec<RunEffect> = order
        .iter()
        .map(|effect| {
            state
                .results
                .remove(effect.name.as_str())
                .unwrap_or_else(|| unreachable!("every ordered effect got a record"))
        })
        .collect();
    journal.state.runs.push(RunRecord {
        recorded_at: unix_now(),
        effects: effects.clone(),
        edges: plan.edges(),
    });
    journal.save()?;

    Ok(RunReport {
        effects,
        orphans: report.orphans,
    })
}

/// Groups effects by longest-path depth; each group only depends on earlier
/// groups.
fn levels<'a>(order: &[&'a Effect]) -> Vec<Vec<&'a Effect>> {
    let mut depth: BTreeMap<&str, usize> = BTreeMap::new();
    let mut groups: Vec<Vec<&'a Effect>> = Vec::new();
    for effect in order {
        let level = effect
            .inputs
            .values()
            .filter_map(|value| match value {
                Value::Ref(r) => depth.get(r.effect.as_str()).map(|d| d + 1),
                Value::Literal(_) => None,
            })
            .max()
            .unwrap_or(0);
        depth.insert(&effect.name, level);
        if groups.len() <= level {
            groups.resize_with(level + 1, Vec::new);
        }
        groups[level].push(effect);
    }
    groups
}

/// Decides each effect of one level: record it as skipped/cached/failed
/// immediately, or return it as a job to run.
fn stage_level<'a>(
    effects: &[&'a Effect],
    decisions: &BTreeMap<&str, &Decision>,
    journal: &Journal,
    registry: &'a Registry,
    state: &mut RunState,
) -> Vec<Job<'a>> {
    let mut jobs = Vec::new();
    for effect in effects {
        let decision = decisions[effect.name.as_str()];
        let mut record = RunEffect {
            name: effect.name.clone(),
            kind: effect.kind.clone(),
            id: decision.id,
            action: Action::Skipped,
            reason: None,
            duration_ms: 0,
            input_signatures: decision.input_signatures.clone(),
        };
        if let Some(blocker) = first_halted_dependency(effect, &state.halted) {
            record.reason = Some(format!("dependency `{blocker}` did not succeed"));
            state.halted.insert(effect.name.clone());
            state.results.insert(effect.name.clone(), record);
            continue;
        }
        if decision.verdict == Verdict::Cached {
            let entry = journal
                .entry(&decision.id)
                .unwrap_or_else(|| unreachable!("cached verdict implies an entry"));
            state
                .outputs
                .insert(effect.name.clone(), entry.outputs.clone());
            record.action = Action::Cached;
            state.results.insert(effect.name.clone(), record);
            continue;
        }
        record.reason = Some(decision.reason.clone());
        match resolve_inputs(effect, &state.outputs) {
            Ok(inputs) => {
                let executor = registry
                    .get(&effect.executor)
                    .unwrap_or_else(|| unreachable!("executors verified before staging"));
                state.results.insert(effect.name.clone(), record);
                jobs.push(Job {
                    effect,
                    executor,
                    request: ExecuteRequest {
                        name: effect.name.clone(),
                        kind: effect.kind.clone(),
                        inputs,
                    },
                });
            }
            Err(message) => {
                record.action = Action::Failed;
                record.reason = Some(message);
                state.halted.insert(effect.name.clone());
                state.results.insert(effect.name.clone(), record);
            }
        }
    }
    jobs
}

/// Folds one level's outcomes into the run state and the journal cache.
fn settle_level(outcomes: Vec<JobOutcome<'_>>, journal: &mut Journal, state: &mut RunState) {
    for outcome in outcomes {
        let name = &outcome.effect.name;
        let record = state
            .results
            .get_mut(name.as_str())
            .unwrap_or_else(|| unreachable!("job records inserted before spawn"));
        record.duration_ms = outcome.execution.duration_ms;
        let (status, action, produced) = match outcome.execution.result {
            Ok(produced) => (Status::Succeeded, Action::Executed, produced),
            Err(err) => {
                record.reason = Some(err.to_string());
                state.halted.insert(name.clone());
                (Status::Failed, Action::Failed, Outputs::new())
            }
        };
        record.action = action;
        journal.state.entries.insert(
            record.id.to_hex(),
            JournalEntry {
                name: name.clone(),
                kind: outcome.effect.kind.clone(),
                outputs: produced.clone(),
                status,
                recorded_at: unix_now(),
            },
        );
        if status == Status::Succeeded {
            state.outputs.insert(name.clone(), produced);
        }
    }
}

fn first_halted_dependency(effect: &Effect, halted: &BTreeSet<String>) -> Option<String> {
    effect.inputs.values().find_map(|value| match value {
        Value::Ref(r) if halted.contains(&r.effect) => Some(r.effect.clone()),
        _ => None,
    })
}

/// Resolves reference inputs against upstream outputs. An `Err` is a user
/// error (the referenced output field does not exist), reported on the
/// effect rather than aborting the run.
fn resolve_inputs(
    effect: &Effect,
    outputs: &BTreeMap<String, Outputs>,
) -> Result<BTreeMap<String, Literal>, String> {
    effect
        .inputs
        .iter()
        .map(|(key, value)| match value {
            Value::Literal(lit) => Ok((key.clone(), lit.clone())),
            Value::Ref(r) => outputs
                .get(&r.effect)
                .and_then(|fields| fields.get(&r.field))
                .cloned()
                .map(|lit| (key.clone(), lit))
                .ok_or_else(|| format!("effect `{}` has no output field `{}`", r.effect, r.field)),
        })
        .collect()
}

/// Runs one level's jobs on parallel threads and collects their outcomes.
/// A panicking executor is reported as that effect's failure.
fn run_level(jobs: Vec<Job<'_>>) -> Vec<JobOutcome<'_>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|job| {
                let effect = job.effect;
                let handle = scope.spawn(move || {
                    let started = Instant::now();
                    let result = job.executor.execute(&job.request);
                    Execution {
                        result,
                        duration_ms: started.elapsed().as_millis(),
                    }
                });
                (effect, handle)
            })
            .collect();
        handles
            .into_iter()
            .map(|(effect, handle)| {
                let execution = handle.join().unwrap_or_else(|_| Execution {
                    result: Err(ExecuteError::new("executor panicked")),
                    duration_ms: 0,
                });
                JobOutcome { effect, execution }
            })
            .collect()
    })
}
