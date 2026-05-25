use std::process::Command;

use serde_json::Value;

fn run_binary(spec: &str) -> (std::process::Output, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("spec.json");
    std::fs::write(&path, spec).unwrap();
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let output = Command::new(bin)
        .arg("--output")
        .arg("json")
        .arg(&path)
        .output()
        .expect("spawn dag-runner");
    (output, dir)
}

fn parse_events(stdout: &[u8]) -> Vec<Value> {
    std::str::from_utf8(stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("ndjson event"))
        .collect()
}

#[test]
fn all_succeed_produces_zero_exit_and_finished_events() {
    let spec = r#"{"nodes":{
        "a":{"command":["true"]},
        "b":{"command":["true"],"depends_on":["a"]}
    }}"#;
    let (output, _dir) = run_binary(spec);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events = parse_events(&output.stdout);

    let summary = events
        .iter()
        .find(|e| e["event"] == "summary")
        .expect("summary event");
    assert_eq!(summary["total"], 2);
    assert_eq!(summary["succeeded"], 2);
    assert_eq!(summary["failed"], 0);
    assert_eq!(summary["skipped"], 0);

    let finished: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "node_finished")
        .collect();
    assert_eq!(finished.len(), 2);
    for ev in finished {
        assert_eq!(ev["outcome"], "succeeded");
        assert!(ev["exit_code"].is_null());
    }
}

#[test]
fn failed_dep_skips_downstream_with_exit_one() {
    let spec = r#"{"nodes":{
        "a":{"command":["false"]},
        "b":{"command":["true"],"depends_on":["a"]}
    }}"#;
    let (output, _dir) = run_binary(spec);
    // `false` exits 1; skipped also contributes 1 → worst is 1.
    assert_eq!(output.status.code(), Some(1));
    let events = parse_events(&output.stdout);
    let summary = events.iter().find(|e| e["event"] == "summary").unwrap();
    assert_eq!(summary["failed"], 1);
    assert_eq!(summary["skipped"], 1);
    let b = events
        .iter()
        .find(|e| e["event"] == "node_finished" && e["node"] == "b")
        .unwrap();
    assert_eq!(b["outcome"], "skipped");
}

#[test]
fn skipped_node_reports_zero_json_duration_after_slow_failed_dep() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","sleep 0.2; false"]},
        "b":{"command":["true"],"depends_on":["a"]}
    }}"#;
    let (output, _dir) = run_binary(spec);
    assert_eq!(output.status.code(), Some(1));
    let events = parse_events(&output.stdout);
    let b = events
        .iter()
        .find(|e| e["event"] == "node_finished" && e["node"] == "b")
        .unwrap();
    assert_eq!(b["outcome"], "skipped");
    assert_eq!(b["duration_ms"], 0);
}

#[test]
fn worst_failure_drives_exit_code() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","exit 3"]},
        "b":{"command":["sh","-c","exit 9"]}
    }}"#;
    let (output, _dir) = run_binary(spec);
    assert_eq!(output.status.code(), Some(9));
}

#[test]
fn env_overlay_is_visible_to_child() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","test \"$DAG_RUNNER_TEST\" = wired"],"env":{"DAG_RUNNER_TEST":"wired"}}
    }}"#;
    let (output, _dir) = run_binary(spec);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn env_overlay_shadows_parent() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","test \"$DAG_RUNNER_TEST\" = child"],"env":{"DAG_RUNNER_TEST":"child"}}
    }}"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spec.json");
    std::fs::write(&path, spec).unwrap();
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let output = Command::new(bin)
        .arg("--output")
        .arg("json")
        .arg(&path)
        .env("DAG_RUNNER_TEST", "parent")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn parent_env_inherited_when_no_overlay() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","test \"$DAG_RUNNER_TEST\" = parent"]}
    }}"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spec.json");
    std::fs::write(&path, spec).unwrap();
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let output = Command::new(bin)
        .arg("--output")
        .arg("json")
        .arg(&path)
        .env("DAG_RUNNER_TEST", "parent")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn env_value_with_equals_is_preserved() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","test \"$DAG_RUNNER_TEST\" = 'a=b=c'"],"env":{"DAG_RUNNER_TEST":"a=b=c"}}
    }}"#;
    let (output, _dir) = run_binary(spec);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn node_with_timeout_kills_long_sleeper_and_exits_124() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","sleep 30"],"timeout_secs":1}
    }}"#;
    // Run in plain mode so the per-node stderr dump appears on the binary's
    // stderr; the JSON event stream summarises but doesn't include captured
    // child output.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spec.json");
    std::fs::write(&path, spec).unwrap();
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let output = Command::new(bin)
        .arg("--output")
        .arg("plain")
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(124));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("timed out after 1s"),
        "expected stderr to mention timeout, got: {stderr}"
    );
}

#[test]
fn node_completes_before_timeout_succeeds() {
    let spec = r#"{"nodes":{
        "a":{"command":["true"],"timeout_secs":30}
    }}"#;
    let (output, _dir) = run_binary(spec);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cycle_is_rejected_before_running() {
    let spec = r#"{"nodes":{
        "a":{"command":["true"],"depends_on":["b"]},
        "b":{"command":["true"],"depends_on":["a"]}
    }}"#;
    let (output, _dir) = run_binary(spec);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cycle"),
        "expected cycle error, got: {stderr}"
    );
}

#[test]
fn sigint_cancels_running_nodes_with_exit_130() {
    use std::thread;
    use std::time::Duration;
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","sleep 30"]}
    }}"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spec.json");
    std::fs::write(&path, spec).unwrap();
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let mut child = std::process::Command::new(bin)
        .arg("--output")
        .arg("plain")
        .arg(&path)
        .spawn()
        .expect("spawn");
    let pid = child.id();
    // Give the runner time to spawn the sleep child and enter its wait.
    thread::sleep(Duration::from_millis(300));
    // SAFETY: we just spawned this child and it has not yet been reaped;
    // SIGINT is a valid signal. libc here avoids any /bin/kill path
    // assumption inside the Nix sandbox.
    let rc = unsafe { libc::kill(pid.cast_signed(), libc::SIGINT) };
    assert_eq!(
        rc,
        0,
        "kill(SIGINT) failed: errno {}",
        std::io::Error::last_os_error()
    );
    let exit = child.wait().expect("wait for runner");
    assert_eq!(exit.code(), Some(130), "expected exit 130 after SIGINT");
}

#[test]
fn missing_dependency_is_rejected_before_running() {
    let spec = r#"{"nodes":{
        "a":{"command":["true"],"depends_on":["ghost"]}
    }}"#;
    let (output, _dir) = run_binary(spec);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ghost"),
        "expected missing-dep error to name 'ghost', got: {stderr}"
    );
}

#[test]
fn empty_command_is_rejected_before_running() {
    let spec = r#"{"nodes":{
        "a":{"command":[]}
    }}"#;
    let (output, _dir) = run_binary(spec);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("empty command"),
        "expected empty-command error, got: {stderr}"
    );
    assert!(
        !stderr.contains("panicked"),
        "empty command should be a validation error: {stderr}"
    );
}

#[test]
fn only_runs_just_the_named_nodes_and_skips_spawning_the_rest() {
    // The dropped node would exit 7 if it ran; success here proves --only
    // filtered it out before spawn rather than just hiding it from the report.
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","exit 7"]},
        "b":{"command":["true"]},
        "c":{"command":["true"]}
    }}"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spec.json");
    std::fs::write(&path, spec).unwrap();
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let output = Command::new(bin)
        .arg("--output")
        .arg("json")
        .arg("--only")
        .arg("b,c")
        .arg(&path)
        .output()
        .expect("spawn dag-runner");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let events = parse_events(&output.stdout);
    let summary = events.iter().find(|e| e["event"] == "summary").unwrap();
    assert_eq!(summary["total"], 2);
    assert_eq!(summary["succeeded"], 2);
    let mut ran: Vec<&str> = events
        .iter()
        .filter(|e| e["event"] == "node_finished")
        .map(|e| e["node"].as_str().unwrap())
        .collect();
    ran.sort_unstable();
    assert_eq!(ran, vec!["b", "c"]);
}
