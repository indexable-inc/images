//! `--ir`: the plan IR JSON entrypoint, fed by the nix efx library.
//!
//! The terranix-port fixture consumed here is the same artifact the nix eval
//! tests (tests/efx-plan.nix) assert the nix emitter renders, so the emitter
//! and this parser are pinned to one document.

use std::path::Path;
use std::process::Command;

const TERRANIX_PORT: &str = include_str!("fixtures/terranix_port.plan.json");

struct Run {
    stdout: String,
    stderr: String,
    success: bool,
}

fn efx(dir: &Path, args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_efx"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("efx binary runs");
    Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
    }
}

#[test]
fn plans_the_nix_emitted_terranix_port() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("plan.json"), TERRANIX_PORT).unwrap();

    let plan = efx(dir.path(), &["plan", "--ir", "plan.json"]);
    assert!(plan.success, "{}{}", plan.stdout, plan.stderr);
    assert!(
        plan.stdout
            .contains("29 effect(s), 29 to execute, 0 cached"),
        "{}",
        plan.stdout
    );
    // The reference chain from the ported interpolations orders the DAG: a
    // zone precedes the records that read its id.
    assert!(
        plan.stdout
            .contains("cloudflare_zone.ix_dev (cloudflare.zone)"),
        "{}",
        plan.stdout
    );
    assert!(
        plan.stdout
            .contains("cloudflare_dns_record.ix_dev_apex (cloudflare.dns_record)"),
        "{}",
        plan.stdout
    );
    let zone = plan
        .stdout
        .find("cloudflare_zone.ix_dev (")
        .expect("zone listed");
    let record = plan
        .stdout
        .find("cloudflare_dns_record.ix_dev_apex (")
        .expect("record listed");
    assert!(zone < record, "zone plans before its dependent record");
}

#[test]
fn ir_documents_default_inputs_and_meta() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("plan.json"),
        r#"{"effects": [{"name": "stamp", "kind": "cmd.run", "executor": "cmd.run",
             "inputs": {"command": {"literal": "echo minimal"}}}]}"#,
    )
    .unwrap();
    let apply = efx(dir.path(), &["apply", "--ir", "plan.json"]);
    assert!(apply.success, "{}{}", apply.stdout, apply.stderr);
    assert!(
        apply.stdout.contains("1 executed, 0 cached"),
        "{}",
        apply.stdout
    );
}

#[test]
fn rejects_duplicate_effect_names_in_ir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("plan.json"),
        r#"{"effects": [
             {"name": "a", "kind": "cmd.run", "executor": "cmd.run"},
             {"name": "a", "kind": "cmd.run", "executor": "cmd.run"}]}"#,
    )
    .unwrap();
    let plan = efx(dir.path(), &["plan", "--ir", "plan.json"]);
    assert!(!plan.success);
    assert!(
        plan.stderr.contains("duplicate effect name `a`"),
        "{}",
        plan.stderr
    );
}

#[test]
fn declared_gap_kinds_fail_loudly_on_apply() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("plan.json"),
        r#"{"effects": [{"name": "betteruptime_monitor.website_cli",
             "kind": "betteruptime.monitor", "executor": "betteruptime.monitor",
             "inputs": {"url": {"literal": "https://ix.dev/"}}}]}"#,
    )
    .unwrap();
    let apply = efx(dir.path(), &["apply", "--ir", "plan.json"]);
    assert!(!apply.success, "a declared gap must fail the apply");
    assert!(
        apply.stdout.contains("is not implemented"),
        "{}",
        apply.stdout
    );
    assert!(
        apply.stdout.contains("NOT applied"),
        "the failure must say the resource was not touched: {}",
        apply.stdout
    );
    assert!(
        apply.stdout.contains("opentofu"),
        "the failure must point at the interim path: {}",
        apply.stdout
    );
}
