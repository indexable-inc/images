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
    fn new(timeout_secs: Option<u64>, leader: LeaderBehavior, streams: DescendantStreams) -> Self {
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
        std::fs::write(
            &spec_path,
            serde_json::to_vec(&spec).expect("serialize spec"),
        )
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
    let fixture = DescendantFixture::new(Some(1), LeaderBehavior::Wait, DescendantStreams::Both);
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("spec.json");
    let pid_path = dir.path().join("node.pid");
    // Wait for the node to say it is running rather than guessing at how long
    // that takes. A fixed sleep raced the runner's own start-up -- SIGINT
    // before the handler is installed kills the runner outright, so the test
    // failed under parallel load and passed alone.
    let spec = serde_json::json!({"nodes": {"a": {
        "command": ["sh", "-c", "echo $$ > \"$DAG_RUNNER_PID_FILE\"; sleep 30"],
        "env": {"DAG_RUNNER_PID_FILE": pid_path.to_str().expect("utf-8 temp path")},
    }}});
    std::fs::write(&path, serde_json::to_vec(&spec).expect("serialize spec")).unwrap();
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let mut child = std::process::Command::new(bin)
        .arg("--output")
        .arg("plain")
        .arg(&path)
        .spawn()
        .expect("spawn");
    let pid = child.id();
    wait_for_pid(&pid_path);
    send_sigint(pid);
    let exit = child.wait().expect("wait for runner");
    assert_eq!(exit.code(), Some(130), "expected exit 130 after SIGINT");
}

#[test]
fn sigint_terminates_descendants_and_closes_captured_pipes() {
    let fixture = DescendantFixture::new(None, LeaderBehavior::Wait, DescendantStreams::Both);
    let mut runner = fixture.spawn();
    let runner_pid = runner.id();
    fixture.wait_until_ready();
    send_sigint(runner_pid);

    let output = wait_for_output(&mut runner);

    assert_eq!(output.status.code(), Some(130));
}

#[test]
fn sigint_after_leader_exit_terminates_descendant_and_closes_captured_pipes() {
    let fixture = DescendantFixture::new(None, LeaderBehavior::Exit, DescendantStreams::Stderr);
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

// --- supervision -----------------------------------------------------------

/// A listener the test owns, so a `tcp` probe has something real to find
/// without the suite depending on a server binary being installed.
fn bound_port() -> ListenerFixture {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    ListenerFixture {
        _listener: listener,
        port,
    }
}

struct ListenerFixture {
    _listener: std::net::TcpListener,
    port: u16,
}

/// A port with nothing behind it: bind, read the number, drop the listener.
/// Racy in principle, but nothing in this suite binds a fixed port.
fn closed_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    listener.local_addr().expect("local addr").port()
}

fn write_spec(dir: &Path, spec: &Value) -> PathBuf {
    let path = dir.join("spec.json");
    std::fs::write(&path, serde_json::to_vec(spec).expect("serialize spec")).expect("write spec");
    path
}

fn run_spec(spec: &Value, args: &[&str]) -> RunResult {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_spec(dir.path(), spec);
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let output = Command::new(bin)
        .args(args)
        .arg(&path)
        .output()
        .expect("spawn dag-runner");
    RunResult { output, _dir: dir }
}

/// A service that stays up until the runner stops it, announcing itself on a
/// line a `log_line` probe can match.
fn banner_service(banner: &str) -> Value {
    serde_json::json!({
        "kind": "service",
        "command": ["sh", "-c", format!("echo {banner}; sleep 120")],
        "ready_when": {"log_line": {"pattern": banner}},
        "ready_timeout_secs": 20,
    })
}

#[test]
fn service_is_ready_before_its_dependent_starts_and_stopped_after_it_finishes() {
    let spec = serde_json::json!({"nodes": {
        "server": banner_service("SERVING"),
        "client": {"command": ["true"], "depends_on": ["server"]},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    assert_success(&output);
    let events = parse_events(&output.stdout);

    let index_of = |event: &str, node: &str| {
        events
            .iter()
            .position(|e| e["event"] == event && e["node"] == node)
            .unwrap_or_else(|| panic!("missing {event} for {node} in {events:?}"))
    };
    assert!(
        index_of("node_ready", "server") < index_of("node_started", "client"),
        "the dependent must not start until the service says it is ready: {events:?}"
    );

    let server = events
        .iter()
        .find(|e| e["event"] == "node_finished" && e["node"] == "server")
        .expect("server finished");
    assert_eq!(server["outcome"], "stopped");
    assert!(
        server["exit_code"].is_null(),
        "a stopped service carries no exit code: {server}"
    );

    let summary = events.iter().find(|e| e["event"] == "summary").unwrap();
    assert_eq!(summary["succeeded"], 1);
    assert_eq!(summary["stopped"], 1);
    assert_eq!(summary["failed"], 0);
}

#[test]
fn tcp_readiness_passes_against_a_listening_port() {
    let listener = bound_port();
    let spec = serde_json::json!({"nodes": {
        "server": {
            "kind": "service",
            "command": ["sh", "-c", "sleep 120"],
            "ready_when": {"tcp": {"address": format!("127.0.0.1:{}", listener.port)}},
            "ready_timeout_secs": 20,
        },
        "client": {"command": ["true"], "depends_on": ["server"]},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    assert_success(&output);
}

#[test]
fn a_service_that_never_becomes_ready_fails_and_its_dependent_never_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The dependent's own side effect is the evidence. Asserting only on the
    // reported outcome would pass just as well if the runner started it and
    // mislabelled the result.
    let ran = dir.path().join("client-ran");
    let spec = serde_json::json!({"nodes": {
        "server": {
            "kind": "service",
            "command": ["sh", "-c", "sleep 120"],
            "ready_when": {"tcp": {"address": format!("127.0.0.1:{}", closed_port())}},
            "ready_timeout_secs": 1,
        },
        "client": {
            "command": ["sh", "-c", format!("touch {}", ran.display())],
            "depends_on": ["server"],
        },
    }});
    let path = write_spec(dir.path(), &spec);
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let output = Command::new(bin)
        .args(["--output", "json"])
        .arg(&path)
        .output()
        .expect("spawn dag-runner");

    assert_eq!(output.status.code(), Some(124), "readiness timeout is 124");
    assert!(
        !ran.exists(),
        "the dependent ran even though its service never came up"
    );
    let events = parse_events(&output.stdout);
    let client = events
        .iter()
        .find(|e| e["event"] == "node_finished" && e["node"] == "client")
        .expect("client finished");
    assert_eq!(client["outcome"], "skipped");
    assert!(
        !events.iter().any(|e| e["event"] == "node_ready"),
        "nothing became ready: {events:?}"
    );
}

#[test]
fn a_service_dying_mid_run_takes_the_group_down() {
    let dir = tempfile::tempdir().expect("tempdir");
    // If the runner failed to notice, the client would run to completion and
    // leave this behind.
    let finished = dir.path().join("client-finished");
    let spec = serde_json::json!({"nodes": {
        "server": {
            "kind": "service",
            "command": ["sh", "-c", "echo SERVING; sleep 0.5; exit 7"],
            "ready_when": {"log_line": {"pattern": "SERVING"}},
            "ready_timeout_secs": 20,
        },
        "client": {
            "command": ["sh", "-c", format!("sleep 60; touch {}", finished.display())],
            "depends_on": ["server"],
        },
    }});
    let path = write_spec(dir.path(), &spec);
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let started = Instant::now();
    let output = Command::new(bin)
        .args(["--output", "json"])
        .arg(&path)
        .output()
        .expect("spawn dag-runner");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "the runner waited out the client's sleep instead of noticing its service had died \
         (took {elapsed:?})"
    );
    assert!(
        !finished.exists(),
        "the client was left running to completion"
    );

    let events = parse_events(&output.stdout);
    let server = events
        .iter()
        .find(|e| e["event"] == "node_finished" && e["node"] == "server")
        .expect("server finished");
    assert_eq!(server["outcome"], "failed");
    assert_eq!(server["exit_code"], 7);

    let client = events
        .iter()
        .find(|e| e["event"] == "node_finished" && e["node"] == "client")
        .expect("client finished");
    assert_eq!(client["outcome"], "failed");
    // 128 + SIGTERM: the client did not fail on its own, the runner stopped it.
    assert_eq!(client["exit_code"], 143);
}

#[test]
fn a_failing_task_still_lets_its_independent_siblings_finish() {
    // The group-wide abort belongs to services. A task failing keeps the
    // pre-supervision behaviour, which is what health-checks depends on: one
    // example fleet blowing up must not cancel the other four.
    let spec = serde_json::json!({"nodes": {
        "doomed": {"command": ["sh", "-c", "exit 3"]},
        "sibling": {"command": ["sh", "-c", "sleep 0.5; exit 0"]},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    assert_eq!(output.status.code(), Some(3));
    let events = parse_events(&output.stdout);
    let sibling = events
        .iter()
        .find(|e| e["event"] == "node_finished" && e["node"] == "sibling")
        .expect("sibling finished");
    assert_eq!(
        sibling["outcome"], "succeeded",
        "a task failure must not cancel an unrelated branch: {events:?}"
    );
}

#[test]
fn a_task_only_spec_reports_exactly_what_it_did_before_services_existed() {
    // health-checks generates specs of this shape and nothing else. Its
    // summary must not grow a service-shaped story.
    let spec = serde_json::json!({"nodes": {
        "a": {"command": ["true"]},
        "b": {"command": ["true"], "depends_on": ["a"]},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    assert_success(&output);
    let events = parse_events(&output.stdout);
    assert!(
        !events.iter().any(|e| e["event"] == "node_ready"),
        "a task never emits node_ready: {events:?}"
    );
    let summary = events.iter().find(|e| e["event"] == "summary").unwrap();
    assert_eq!(summary["total"], 2);
    assert_eq!(summary["succeeded"], 2);
    assert_eq!(summary["stopped"], 0);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("stopped"),
        "the human summary should not mention stopped when nothing was: {stderr}"
    );
}

#[test]
fn retained_output_is_bounded_and_says_so_when_it_truncates() {
    let spec = serde_json::json!({"nodes": {
        "loud": {"command": ["sh", "-c", "i=0; while [ $i -lt 2000 ]; do echo line-$i; i=$((i+1)); done; exit 1"]},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "plain"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("earlier lines dropped"),
        "a truncated dump must say it was truncated: {stderr}"
    );
    assert!(
        stderr.contains("line-1999"),
        "the newest lines are the ones worth keeping: {stderr}"
    );
    assert!(
        !stderr.contains("line-0\n"),
        "the oldest lines should have fallen off the front: {stderr}"
    );
}

#[test]
fn lifeline_fd_closes_when_the_runner_is_killed_outright() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_path = dir.path().join("node.pid");
    let saw_eof = dir.path().join("saw-eof");
    // `cat <&3` blocks until the write end closes. The runner is SIGKILLed, so
    // no teardown of any kind runs: the only thing that can release this child
    // is the pipe dying with the runner process.
    let spec = serde_json::json!({"nodes": {"held": {
        "command": ["sh", "-c", format!(
            "echo $$ > {}; cat <&3; touch {}",
            pid_path.display(),
            saw_eof.display(),
        )],
        "lifeline_fd": 3,
    }}});
    let path = write_spec(dir.path(), &spec);
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let mut runner = Command::new(bin)
        .args(["--output", "plain"])
        .arg(&path)
        .spawn()
        .expect("spawn dag-runner");

    wait_for_pid(&pid_path);
    // SAFETY: the runner was just spawned and has not been reaped; SIGKILL is
    // valid, and killing it outright is the case under test.
    unsafe {
        libc::kill(runner.id().cast_signed(), libc::SIGKILL);
    }
    let _ = runner.wait();

    let deadline = Instant::now() + Duration::from_secs(5);
    while !saw_eof.exists() {
        assert!(
            Instant::now() < deadline,
            "the child never saw EOF on its lifeline; it is orphaned"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn without_a_lifeline_the_same_child_is_orphaned() {
    // The control for the test above. Without it, that test would pass just as
    // well if `sh` happened to exit for some unrelated reason.
    let dir = tempfile::tempdir().expect("tempdir");
    let pid_path = dir.path().join("node.pid");
    let spec = serde_json::json!({"nodes": {"held": {
        "command": ["sh", "-c", format!("echo $$ > {}; sleep 30", pid_path.display())],
    }}});
    let path = write_spec(dir.path(), &spec);
    let bin = env!("CARGO_BIN_EXE_dag-runner");
    let mut runner = Command::new(bin)
        .args(["--output", "plain"])
        .arg(&path)
        .spawn()
        .expect("spawn dag-runner");

    wait_for_pid(&pid_path);
    let child_pid: libc::pid_t = std::fs::read_to_string(&pid_path)
        .expect("read pid")
        .trim()
        .parse()
        .expect("parse pid");
    // SAFETY: as above.
    unsafe {
        libc::kill(runner.id().cast_signed(), libc::SIGKILL);
    }
    let _ = runner.wait();
    thread::sleep(Duration::from_millis(500));

    // SAFETY: signal 0 only checks reachability; it delivers nothing.
    let alive = unsafe { libc::kill(child_pid, 0) } == 0;
    // SAFETY: cleaning up the orphan this test deliberately created.
    unsafe {
        libc::kill(child_pid, libc::SIGKILL);
    }
    assert!(
        alive,
        "the child died without a lifeline, so the lifeline test proves nothing"
    );
}

#[test]
fn a_service_without_a_readiness_probe_is_rejected() {
    let spec = serde_json::json!({"nodes": {
        "s": {"kind": "service", "command": ["true"]},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ready_when"),
        "the error should name the missing field: {stderr}"
    );
}

#[test]
fn a_task_with_a_readiness_probe_is_rejected() {
    let spec = serde_json::json!({"nodes": {
        "t": {"command": ["true"], "ready_when": {"tcp": {"address": "127.0.0.1:1"}}},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ready_when"), "got: {stderr}");
}

#[test]
fn log_line_readiness_against_an_inherited_stream_is_rejected() {
    let spec = serde_json::json!({"nodes": {
        "s": {
            "kind": "service",
            "command": ["true"],
            "stdio": "inherit",
            "ready_when": {"log_line": {"pattern": "x"}},
        },
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "plain"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("inherit"), "got: {stderr}");
}

#[test]
fn json_output_refuses_an_inheriting_node_rather_than_letting_it_corrupt_the_stream() {
    let spec = serde_json::json!({"nodes": {
        "t": {"command": ["true"], "stdio": "inherit"},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("NDJSON") && stderr.contains('t'),
        "the error should name the node and the reason: {stderr}"
    );
}

#[test]
fn a_lifeline_on_the_childs_own_stdio_is_rejected() {
    let spec = serde_json::json!({"nodes": {
        "t": {"command": ["true"], "lifeline_fd": 1},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("lifeline_fd"), "got: {stderr}");
}

#[test]
fn a_misspelled_field_is_rejected_rather_than_ignored() {
    // A silently dropped `ready_when` would leave a service that never becomes
    // ready and a spec that looks correct.
    let spec = serde_json::json!({"nodes": {
        "s": {"kind": "service", "command": ["true"], "readywhen": {"tcp": {"address": "127.0.0.1:1"}}},
    }});
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("readywhen") && stderr.contains("ready_when"),
        "the error should name the typo and the field it was probably meant to be: {stderr}"
    );
}

#[test]
fn a_grandchild_holding_the_pipe_cannot_hide_a_dead_service() {
    // The regression this exists for: the runner used to learn a child had
    // exited only by draining its streams first, and a backgrounded
    // grandchild inherits stdout and holds it open. A service that exited 5
    // immediately was reported `stopped`, the client ran its full 90 seconds,
    // and the run exited 0. A dead dependency reported as a clean shutdown is
    // the worst answer available, so this asserts the outcome and the code,
    // not just the latency.
    let spec = serde_json::json!({"nodes": {
        "server": {
            "kind": "service",
            // `sleep 120 &` keeps the inherited stdout open after the leader
            // is gone. That is the whole point of the fixture.
            "command": ["sh", "-c", "sleep 120 & echo READY; exit 5"],
            "ready_when": {"log_line": {"pattern": "READY"}},
            "ready_timeout_secs": 20,
        },
        "client": {"command": ["sh", "-c", "sleep 90"], "depends_on": ["server"]},
    }});
    let started = Instant::now();
    let RunResult { output, _dir } = run_spec(&spec, &["--output", "json"]);
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "the dead service went unnoticed until the client finished ({elapsed:?})"
    );
    let events = parse_events(&output.stdout);
    let server = events
        .iter()
        .find(|e| e["event"] == "node_finished" && e["node"] == "server")
        .expect("server finished");
    assert_eq!(
        server["outcome"], "failed",
        "a service that exited must not be reported as one the runner stopped: {events:?}"
    );
    assert_eq!(
        server["exit_code"], 5,
        "the service's own code, not a signal"
    );
    assert_ne!(output.status.code(), Some(0), "the run must not exit clean");
}
