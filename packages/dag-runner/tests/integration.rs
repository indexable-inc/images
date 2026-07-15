use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// The process output of one `dag-runner` invocation. Holds the temp dir so it
/// outlives the run; the dir is deleted when this is dropped.
struct RunResult {
    output: std::process::Output,
    _dir: tempfile::TempDir,
}

fn run_binary(spec: &str) -> RunResult {
    run_binary_configured(spec, |_| {})
}

fn run_binary_configured(spec: &str, configure: impl FnOnce(&mut Command)) -> RunResult {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("spec.json");
    std::fs::write(&path, spec).unwrap();
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let mut command = Command::new(bin);
    command.arg("--output").arg("json").arg(&path);
    configure(&mut command);
    let output = command.output().expect("spawn dag-runner");
    RunResult { output, _dir: dir }
}

fn run_binary_with_env(spec: &str, key: &str, value: &str) -> RunResult {
    run_binary_configured(spec, |command| {
        command.env(key, value);
    })
}

fn parse_events(stdout: &[u8]) -> Vec<Value> {
    std::str::from_utf8(stdout)
        .unwrap()
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("ndjson event"))
        .collect()
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct DescendantFixture {
    _dir: tempfile::TempDir,
    spec_path: PathBuf,
    child_pid_path: PathBuf,
    descendant_pid_path: PathBuf,
}

#[derive(Clone, Copy)]
enum LeaderBehavior {
    Wait,
    Exit,
}

#[derive(Clone, Copy)]
enum DescendantStreams {
    Both,
    Stderr,
}

impl DescendantFixture {
    fn new(
        timeout_secs: Option<u64>,
        leader: LeaderBehavior,
        streams: DescendantStreams,
    ) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let script_path = dir.path().join("process-tree.sh");
        let spec_path = dir.path().join("spec.json");
        let child_pid_path = dir.path().join("child.pid");
        let descendant_pid_path = dir.path().join("descendant.pid");
        let final_command = match leader {
            LeaderBehavior::Wait => "wait",
            LeaderBehavior::Exit => "exit 0",
        };
        let close_stdout = match streams {
            DescendantStreams::Both => "",
            DescendantStreams::Stderr => "exec >&-",
        };
        let script = format!(
            r#"trap '' TERM
printf '%s\n' "$$" > "$DAG_RUNNER_CHILD_PID"
sh -c '
  trap "" TERM
  printf "%s\n" "$$" > "$DAG_RUNNER_DESCENDANT_PID"
  {close_stdout}
  exec sleep 10
' &
{final_command}
"#
        );
        std::fs::write(&script_path, script).expect("write process tree fixture");

        let mut node = serde_json::json!({
            "command": ["sh", script_path],
            "env": {
                "DAG_RUNNER_CHILD_PID": child_pid_path,
                "DAG_RUNNER_DESCENDANT_PID": descendant_pid_path,
            },
        });
        if let Some(secs) = timeout_secs {
            node["timeout_secs"] = serde_json::json!(secs);
        }
        let spec = serde_json::json!({"nodes": {"process-tree": node}});
        std::fs::write(&spec_path, serde_json::to_vec(&spec).expect("serialize spec"))
            .expect("write spec");

        Self {
            _dir: dir,
            spec_path,
            child_pid_path,
            descendant_pid_path,
        }
    }

    fn spawn(&self) -> RunningFixture {
        let bin = env!("CARGO_BIN_EXE_dag-runner");
        let mut command = Command::new(bin);
        command
            .args(["--output", "plain"])
            .arg(&self.spec_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        let child = command.spawn().expect("spawn dag-runner");
        RunningFixture {
            process_group: child.id().cast_signed(),
            child,
            armed: true,
        }
    }

    fn wait_until_ready(&self) {
        wait_for_pid(&self.child_pid_path);
        wait_for_pid(&self.descendant_pid_path);
    }
}

struct RunningFixture {
    child: Child,
    process_group: libc::pid_t,
    armed: bool,
}

impl RunningFixture {
    fn id(&self) -> u32 {
        self.child.id()
    }

    const fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RunningFixture {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // SAFETY: the runner was spawned as the leader of this process group,
        // and SIGKILL is valid. A live fixture member keeps the group ID owned.
        unsafe {
            libc::killpg(self.process_group, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

fn wait_for_pid(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && text.trim().parse::<libc::pid_t>().is_ok()
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "process did not record its PID at {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_output(runner: &mut RunningFixture) -> Output {
    let deadline = Instant::now() + Duration::from_secs(4);
    let status = loop {
        if let Some(status) = runner.child.try_wait().expect("poll dag-runner") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "dag-runner did not return after terminating its node"
        );
        thread::sleep(Duration::from_millis(10));
    };
    runner.disarm();

    let mut stdout = Vec::new();
    runner
        .child
        .stdout
        .take()
        .expect("stdout piped")
        .read_to_end(&mut stdout)
        .expect("read stdout");
    let mut stderr = Vec::new();
    runner
        .child
        .stderr
        .take()
        .expect("stderr piped")
        .read_to_end(&mut stderr)
        .expect("read stderr");
    Output {
        status,
        stdout,
        stderr,
    }
}

fn send_sigint(pid: u32) {
    // SAFETY: the runner was just spawned and has not been reaped; SIGINT is
    // valid and exercises the public cancellation path.
    let rc = unsafe { libc::kill(pid.cast_signed(), libc::SIGINT) };
    assert_eq!(
        rc,
        0,
        "kill(SIGINT) failed: errno {}",
        std::io::Error::last_os_error()
    );
}

#[test]
fn all_succeed_produces_zero_exit_and_finished_events() {
    let spec = r#"{"nodes":{
        "a":{"command":["true"]},
        "b":{"command":["true"],"depends_on":["a"]}
    }}"#;
    let RunResult { output, _dir } = run_binary(spec);
    assert_success(&output);
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
    let RunResult { output, _dir } = run_binary(spec);
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
    let RunResult { output, _dir } = run_binary(spec);
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
    let RunResult { output, _dir } = run_binary(spec);
    assert_eq!(output.status.code(), Some(9));
}

#[test]
fn env_overlay_is_visible_to_child() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","test \"$DAG_RUNNER_TEST\" = wired"],"env":{"DAG_RUNNER_TEST":"wired"}}
    }}"#;
    let RunResult { output, _dir } = run_binary(spec);
    assert_success(&output);
}

#[test]
fn env_overlay_shadows_parent() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","test \"$DAG_RUNNER_TEST\" = child"],"env":{"DAG_RUNNER_TEST":"child"}}
    }}"#;
    let RunResult { output, _dir } = run_binary_with_env(spec, "DAG_RUNNER_TEST", "parent");
    assert_success(&output);
}

#[test]
fn parent_env_inherited_when_no_overlay() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","test \"$DAG_RUNNER_TEST\" = parent"]}
    }}"#;
    let RunResult { output, _dir } = run_binary_with_env(spec, "DAG_RUNNER_TEST", "parent");
    assert_success(&output);
}

#[test]
fn env_value_with_equals_is_preserved() {
    let spec = r#"{"nodes":{
        "a":{"command":["sh","-c","test \"$DAG_RUNNER_TEST\" = 'a=b=c'"],"env":{"DAG_RUNNER_TEST":"a=b=c"}}
    }}"#;
    let RunResult { output, _dir } = run_binary(spec);
    assert_success(&output);
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
fn timeout_terminates_descendants_and_closes_captured_pipes() {
    let fixture = DescendantFixture::new(
        Some(1),
        LeaderBehavior::Wait,
        DescendantStreams::Both,
    );
    let mut runner = fixture.spawn();
    fixture.wait_until_ready();

    let output = wait_for_output(&mut runner);

    assert_eq!(output.status.code(), Some(124));
}

#[test]
fn node_completes_before_timeout_succeeds() {
    let spec = r#"{"nodes":{
        "a":{"command":["true"],"timeout_secs":30}
    }}"#;
    let RunResult { output, _dir } = run_binary(spec);
    assert_success(&output);
}

#[test]
fn cycle_is_rejected_before_running() {
    let spec = r#"{"nodes":{
        "a":{"command":["true"],"depends_on":["b"]},
        "b":{"command":["true"],"depends_on":["a"]}
    }}"#;
    let RunResult { output, _dir } = run_binary(spec);
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
    send_sigint(pid);
    let exit = child.wait().expect("wait for runner");
    assert_eq!(exit.code(), Some(130), "expected exit 130 after SIGINT");
}

#[test]
fn sigint_terminates_descendants_and_closes_captured_pipes() {
    let fixture = DescendantFixture::new(
        None,
        LeaderBehavior::Wait,
        DescendantStreams::Both,
    );
    let mut runner = fixture.spawn();
    let runner_pid = runner.id();
    fixture.wait_until_ready();
    send_sigint(runner_pid);

    let output = wait_for_output(&mut runner);

    assert_eq!(output.status.code(), Some(130));
}

#[test]
fn sigint_after_leader_exit_terminates_descendant_and_closes_captured_pipes() {
    let fixture = DescendantFixture::new(
        None,
        LeaderBehavior::Exit,
        DescendantStreams::Stderr,
    );
    let mut runner = fixture.spawn();
    let runner_pid = runner.id();
    fixture.wait_until_ready();
    thread::sleep(Duration::from_millis(200));
    send_sigint(runner_pid);

    let output = wait_for_output(&mut runner);

    assert_eq!(output.status.code(), Some(130));
}

#[test]
fn missing_dependency_is_rejected_before_running() {
    let spec = r#"{"nodes":{
        "a":{"command":["true"],"depends_on":["ghost"]}
    }}"#;
    let RunResult { output, _dir } = run_binary(spec);
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
    let RunResult { output, _dir } = run_binary(spec);
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
    assert_success(&output);

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
