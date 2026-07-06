use std::collections::BTreeMap;
use std::sync::Mutex;

use efx_engine::{
    Action, ExecuteError, ExecuteRequest, Executor, Journal, Outputs, Registry, Verdict, apply,
    plan,
};
use efx_ir::{Effect, EffectMeta, Literal, OutputRef, Plan, Value};

/// Records every request it serves and echoes inputs back as outputs.
#[derive(Default)]
struct EchoExecutor {
    calls: Mutex<Vec<String>>,
    fail_on: Option<String>,
}

impl Executor for EchoExecutor {
    fn execute(&self, request: &ExecuteRequest) -> Result<Outputs, ExecuteError> {
        self.calls
            .lock()
            .expect("no poisoned lock in tests")
            .push(request.name.clone());
        if self.fail_on.as_deref() == Some(request.name.as_str()) {
            return Err(ExecuteError::new("boom"));
        }
        let mut outputs = request.inputs.clone();
        outputs.insert("echo".to_owned(), Literal::Str(request.name.clone()));
        Ok(outputs)
    }
}

fn effect(name: &str, inputs: &[(&str, Value)]) -> Effect {
    Effect {
        name: name.into(),
        kind: "echo".into(),
        executor: "echo".into(),
        inputs: inputs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
        meta: EffectMeta::default(),
    }
}

fn lit(s: &str) -> Value {
    Value::Literal(Literal::Str(s.into()))
}

fn reference(effect: &str, field: &str) -> Value {
    Value::Ref(OutputRef {
        effect: effect.into(),
        field: field.into(),
    })
}

fn chain_plan(seed: &str) -> Plan {
    let mut plan = Plan::new();
    plan.add(effect("src", &[("value", lit(seed))])).unwrap();
    plan.add(effect("mid", &[("value", reference("src", "echo"))]))
        .unwrap();
    plan.add(effect("out", &[("value", reference("mid", "echo"))]))
        .unwrap();
    plan.add(effect("solo", &[("value", lit("constant"))]))
        .unwrap();
    plan
}

fn registry() -> Registry {
    let mut registry = Registry::new();
    registry.register("echo", Box::new(EchoExecutor::default()));
    registry
}

fn journal_at(dir: &tempfile::TempDir) -> Journal {
    Journal::load(dir.path().join("journal.json")).expect("fresh journal loads")
}

fn actions(report: &efx_engine::RunReport) -> BTreeMap<String, Action> {
    report
        .effects
        .iter()
        .map(|e| (e.name.clone(), e.action))
        .collect()
}

#[test]
fn first_apply_executes_second_is_all_cache_hits() {
    let dir = tempfile::tempdir().unwrap();
    let plan_v1 = chain_plan("v1");
    let registry = registry();

    let mut journal = journal_at(&dir);
    let first = apply(&plan_v1, &mut journal, &registry).unwrap();
    assert_eq!(first.count(Action::Executed), 4);
    assert_eq!(first.count(Action::Cached), 0);

    // Reload from disk: the cache must survive the process boundary.
    let mut journal = journal_at(&dir);
    let second = apply(&plan_v1, &mut journal, &registry).unwrap();
    assert_eq!(second.count(Action::Executed), 0);
    assert_eq!(second.count(Action::Cached), 4);
}

#[test]
fn changed_input_invalidates_only_dependents() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry();
    let mut journal = journal_at(&dir);
    apply(&chain_plan("v1"), &mut journal, &registry).unwrap();

    let mut journal = journal_at(&dir);
    let report = apply(&chain_plan("v2"), &mut journal, &registry).unwrap();
    let actions = actions(&report);
    assert_eq!(actions["src"], Action::Executed);
    assert_eq!(actions["mid"], Action::Executed);
    assert_eq!(actions["out"], Action::Executed);
    assert_eq!(
        actions["solo"],
        Action::Cached,
        "untouched branch stays cached"
    );
}

#[test]
fn plan_is_deterministic_and_explains_invalidation() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry();
    let mut journal = journal_at(&dir);
    apply(&chain_plan("v1"), &mut journal, &registry).unwrap();

    let journal = journal_at(&dir);
    let plan_v2 = chain_plan("v2");
    let first = plan(&plan_v2, &journal).unwrap();
    let second = plan(&plan_v2, &journal).unwrap();
    let render = |report: &efx_engine::PlanReport| {
        report
            .decisions
            .iter()
            .map(|d| format!("{} {:?} {} {}", d.name, d.verdict, d.id, d.reason))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        render(&first),
        render(&second),
        "same plan + journal, same output"
    );

    let by_name: BTreeMap<&str, &efx_engine::Decision> = first
        .decisions
        .iter()
        .map(|d| (d.name.as_str(), d))
        .collect();
    assert_eq!(by_name["src"].verdict, Verdict::Execute);
    assert_eq!(by_name["src"].reason, "input `value` changed");
    assert_eq!(by_name["mid"].verdict, Verdict::Execute);
    assert_eq!(by_name["mid"].reason, "upstream `src` changed");
    assert_eq!(by_name["solo"].verdict, Verdict::Cached);
}

#[test]
fn orphans_are_reported_not_destroyed() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry();
    let mut journal = journal_at(&dir);
    apply(&chain_plan("v1"), &mut journal, &registry).unwrap();

    let mut smaller = Plan::new();
    smaller
        .add(effect("solo", &[("value", lit("constant"))]))
        .unwrap();
    let journal = journal_at(&dir);
    let report = plan(&smaller, &journal).unwrap();
    let orphan_names: Vec<&str> = report.orphans.iter().map(|o| o.name.as_str()).collect();
    assert!(orphan_names.contains(&"src"));
    assert!(orphan_names.contains(&"out"));
    assert_eq!(journal.state.entries.len(), 4, "entries untouched");
}

#[test]
fn failure_skips_dependents_but_not_siblings() {
    let dir = tempfile::tempdir().unwrap();
    let mut registry = Registry::new();
    registry.register(
        "echo",
        Box::new(EchoExecutor {
            fail_on: Some("mid".to_owned()),
            ..EchoExecutor::default()
        }),
    );
    let mut journal = journal_at(&dir);
    let report = apply(&chain_plan("v1"), &mut journal, &registry).unwrap();
    let actions = actions(&report);
    assert_eq!(actions["src"], Action::Executed);
    assert_eq!(actions["mid"], Action::Failed);
    assert_eq!(actions["out"], Action::Skipped);
    assert_eq!(actions["solo"], Action::Executed);
    assert!(!report.succeeded());
}

#[test]
fn reference_outputs_flow_between_effects() {
    let dir = tempfile::tempdir().unwrap();
    let registry = registry();
    let mut journal = journal_at(&dir);
    apply(&chain_plan("v1"), &mut journal, &registry).unwrap();
    let ids = chain_plan("v1").effect_ids().unwrap();
    let out_entry = journal.entry(&ids["out"]).unwrap();
    // `out.value` came from `mid.echo`, which is the literal name "mid".
    assert_eq!(out_entry.outputs["value"], Literal::Str("mid".into()));
}
