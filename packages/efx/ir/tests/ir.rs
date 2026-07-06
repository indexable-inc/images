use std::collections::BTreeMap;

use efx_ir::{Effect, EffectMeta, Literal, OutputRef, Plan, PlanError, Value};

fn effect(name: &str, kind: &str, inputs: &[(&str, Value)]) -> Effect {
    Effect {
        name: name.into(),
        kind: kind.into(),
        executor: kind.into(),
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

fn ids_of(plan: &Plan) -> BTreeMap<String, efx_ir::EffectId> {
    plan.effect_ids().expect("valid plan")
}

#[test]
fn hash_is_stable_across_processes() {
    let mut plan = Plan::new();
    plan.add(effect("a", "file.write", &[("content", lit("hello"))]))
        .unwrap();
    let ids = ids_of(&plan);
    // Golden value: the identity must never drift without an intentional
    // format change, or every journal in the wild silently invalidates.
    assert_eq!(
        ids["a"].to_hex(),
        "e8c560d4d99134660d56a14fe244e067ef281334ce4f3b08e119fe756d57ad01"
    );
}

#[test]
fn identity_ignores_name_and_meta() {
    let mut left = Plan::new();
    left.add(effect("a", "file.write", &[("content", lit("x"))]))
        .unwrap();
    let mut right = Plan::new();
    let mut renamed = effect("b", "file.write", &[("content", lit("x"))]);
    renamed.meta.idempotent = false;
    renamed.meta.rollback_hint = Some("undo".into());
    right.add(renamed).unwrap();
    assert_eq!(ids_of(&left)["a"], ids_of(&right)["b"]);
}

#[test]
fn changed_input_changes_identity() {
    let mut before = Plan::new();
    before
        .add(effect("a", "file.write", &[("content", lit("v1"))]))
        .unwrap();
    let mut after = Plan::new();
    after
        .add(effect("a", "file.write", &[("content", lit("v2"))]))
        .unwrap();
    assert_ne!(ids_of(&before)["a"], ids_of(&after)["a"]);
}

#[test]
fn str_and_int_literals_hash_differently() {
    assert_ne!(
        Literal::Str("1".into()).content_hash(),
        Literal::Int(1).content_hash()
    );
}

#[test]
fn invalidation_propagates_through_references() {
    let build = |content: &str| {
        let mut plan = Plan::new();
        plan.add(effect("src", "file.write", &[("content", lit(content))]))
            .unwrap();
        plan.add(effect(
            "mid",
            "html.render",
            &[("template", reference("src", "path"))],
        ))
        .unwrap();
        plan.add(effect(
            "out",
            "file.write",
            &[("content", reference("mid", "html"))],
        ))
        .unwrap();
        plan.add(effect("free", "cmd.run", &[("command", lit("true"))]))
            .unwrap();
        ids_of(&plan)
    };
    let before = build("v1");
    let after = build("v2");
    assert_ne!(before["src"], after["src"]);
    assert_ne!(before["mid"], after["mid"], "transitive dependent moved");
    assert_ne!(before["out"], after["out"], "transitive dependent moved");
    assert_eq!(before["free"], after["free"], "independent effect stable");
}

#[test]
fn unknown_reference_is_rejected() {
    let mut plan = Plan::new();
    plan.add(effect(
        "a",
        "file.write",
        &[("content", reference("ghost", "x"))],
    ))
    .unwrap();
    assert!(matches!(
        plan.effect_ids(),
        Err(PlanError::UnknownReference { .. })
    ));
}

#[test]
fn cycle_is_rejected() {
    let mut plan = Plan::new();
    plan.add(effect(
        "a",
        "file.write",
        &[("content", reference("b", "x"))],
    ))
    .unwrap();
    plan.add(effect(
        "b",
        "file.write",
        &[("content", reference("a", "x"))],
    ))
    .unwrap();
    assert!(matches!(plan.effect_ids(), Err(PlanError::Cycle { .. })));
}

#[test]
fn duplicate_name_is_rejected() {
    let mut plan = Plan::new();
    plan.add(effect("a", "file.write", &[])).unwrap();
    assert!(matches!(
        plan.add(effect("a", "cmd.run", &[])),
        Err(PlanError::DuplicateName { .. })
    ));
}

#[test]
fn edges_derive_from_references() {
    let mut plan = Plan::new();
    plan.add(effect("src", "cmd.run", &[("command", lit("true"))]))
        .unwrap();
    plan.add(effect(
        "out",
        "file.write",
        &[
            ("content", reference("src", "stdout")),
            ("mode", reference("src", "status")),
        ],
    ))
    .unwrap();
    let edges = plan.edges();
    assert_eq!(edges.len(), 1, "duplicate edges collapse");
    assert_eq!(edges[0].from, "src");
    assert_eq!(edges[0].to, "out");
}

#[test]
fn json_round_trip_preserves_identity() {
    let mut plan = Plan::new();
    plan.add(effect("src", "cmd.run", &[("command", lit("true"))]))
        .unwrap();
    plan.add(effect(
        "out",
        "file.write",
        &[("content", reference("src", "stdout"))],
    ))
    .unwrap();
    let json = serde_json::to_string(&plan).unwrap();
    let parsed: Plan = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, plan);
    assert_eq!(ids_of(&parsed), ids_of(&plan));
}

#[test]
fn deserialization_enforces_name_uniqueness() {
    let doc = r#"{"effects": [
        {"name": "a", "kind": "cmd.run", "executor": "cmd.run"},
        {"name": "a", "kind": "cmd.run", "executor": "cmd.run"}]}"#;
    let err = serde_json::from_str::<Plan>(doc).unwrap_err();
    assert!(
        err.to_string().contains("duplicate effect name `a`"),
        "{err}"
    );
}

#[test]
fn minimal_ir_effect_defaults_inputs_and_meta() {
    let doc = r#"{"effects": [{"name": "a", "kind": "cmd.run", "executor": "cmd.run"}]}"#;
    let plan: Plan = serde_json::from_str(doc).unwrap();
    let parsed = plan.get("a").unwrap();
    assert!(parsed.inputs.is_empty());
    assert_eq!(parsed.meta, EffectMeta::default());
}
