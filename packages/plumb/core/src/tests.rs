//! End-to-end shell behavior. External commands are limited to coreutils
//! and `sh`, which the nix check sandbox provides on PATH.

use crate::{Config, Error, Shell};

fn shell() -> Shell {
    Shell::new(Config::default()).expect("shell")
}

fn echoing() -> Shell {
    Shell::new(Config {
        echo_output: false,
        ..Config::default()
    })
    .expect("shell")
}

#[test]
fn pipeline_captures_every_stage() {
    let sh = shell();
    let report = sh.eval("echo hello | tr a-z A-Z").expect("eval");
    assert_eq!(report.status, 0);
    assert_eq!(report.output(), "HELLO\n");
    let stages = &report.pipelines[0].stages;
    assert_eq!(stages.len(), 2);
    assert_eq!(stages[0].stdout.render(), "hello\n");
    assert_eq!(stages[1].stdout.render(), "HELLO\n");
    // The run is addressable afterwards, including intermediate pipe data.
    assert_eq!(sh.var("o").as_deref(), Some("HELLO"));
    let reuse = sh
        .eval("echo ${o[1][0]} then ${o[1]} exit ${s[1]}")
        .expect("reuse");
    assert_eq!(reuse.output(), "hello then HELLO exit 0\n");
}

#[test]
fn pipefail_reports_upstream_failure() {
    let report = shell().eval("sh -c 'exit 3' | cat").expect("eval");
    assert_eq!(report.status, 3);
}

#[test]
fn sigpipe_death_is_not_a_failure() {
    // `yes | head` must terminate (backpressure + SIGPIPE) and count as
    // success even though `yes` died of SIGPIPE.
    let report = shell().eval("yes | head -n 2").expect("eval");
    assert_eq!(report.output(), "y\ny\n");
    assert_eq!(report.status, 0);
    assert_eq!(report.pipelines[0].stages[0].status, 141);
}

#[test]
fn stderr_capture_and_merge() {
    let sh = shell();
    let report = sh.eval("sh -c 'echo oops >&2'").expect("eval");
    assert_eq!(report.pipelines[0].stages[0].stderr.render(), "oops\n");
    assert_eq!(sh.var("e").as_deref(), Some("oops"));
    let reuse = sh.eval("echo saw:${e[1]}").expect("reuse");
    assert_eq!(reuse.output(), "saw:oops\n");

    let merged = sh.eval("sh -c 'echo mixed >&2' 2>&1 | cat").expect("eval");
    let stages = &merged.pipelines[0].stages;
    assert!(stages[0].stderr_merged);
    assert_eq!(stages[1].stdout.render(), "mixed\n");
}

#[test]
fn redirections_write_and_read_files() {
    let dir = std::env::temp_dir().join(format!("plumb-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let sh = Shell::new(Config {
        cwd: Some(dir.clone()),
        ..Config::default()
    })
    .expect("shell");
    sh.eval("echo first > f.txt").expect("write");
    sh.eval("echo second >> f.txt").expect("append");
    let report = sh.eval("cat < f.txt").expect("read");
    assert_eq!(report.output(), "first\nsecond\n");
    // The redirected stream was still captured and stays addressable.
    let captured = sh.eval("echo ${o[1]}").expect("reuse");
    assert_eq!(captured.output(), "first\n");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn command_not_found_is_status_127() {
    let report = shell()
        .eval("definitely-not-a-command-xyz")
        .expect("eval");
    assert_eq!(report.status, 127);
    assert!(
        report.pipelines[0].stages[0]
            .stderr
            .render()
            .contains("command not found")
    );
}

#[test]
fn strictness_errors() {
    let sh = shell();
    assert!(matches!(
        sh.eval("echo $DEFINITELY_UNSET_VARIABLE"),
        Err(Error::UnsetVar { .. })
    ));
    // The aborted run is still committed for inspection.
    let report = sh.reports().pop().expect("committed");
    assert!(report.aborted.is_some());
    assert!(matches!(
        sh.eval("cat /nonexistent-dir-xyz/*.rs"),
        Err(Error::GlobNoMatch { .. })
    ));
}

#[test]
fn command_substitution_feeds_arguments() {
    let sh = shell();
    let report = sh.eval("echo prefix-$(echo -n inner)").expect("eval");
    assert_eq!(report.output(), "prefix-inner\n");
    assert_eq!(report.substitutions.len(), 1);
    assert_eq!(report.substitutions[0].output(), "inner");
}

#[test]
fn auto_bound_variables_cross_runs() {
    let sh = shell();
    sh.eval("echo alpha | tr a-z A-Z").expect("first");
    let report = sh.eval("echo ${o[1]} and ${o[1][0]}").expect("second");
    assert_eq!(report.output(), "ALPHA and alpha\n");
}

#[test]
fn last_status_variable() {
    let sh = shell();
    drop(sh.eval("sh -c 'exit 5'").expect("first"));
    let report = sh.eval("echo $?").expect("second");
    assert_eq!(report.output(), "5\n");
}

#[test]
fn connectors_short_circuit() {
    let report = shell().eval("false && echo skipped; echo ran").expect("eval");
    // `echo skipped` never ran: false, then `echo ran`.
    assert_eq!(report.pipelines.len(), 2);
    assert_eq!(report.output(), "ran\n");

    let fallback = shell().eval("false || echo rescued").expect("eval");
    assert_eq!(fallback.output(), "rescued\n");
}

#[test]
fn assignment_and_environment() {
    let sh = shell();
    sh.eval("GREETING=hi").expect("assign");
    assert_eq!(sh.var("GREETING").as_deref(), Some("hi"));
    // Not exported: children do not see it.
    let unexported = sh.eval("sh -c 'echo value:$GREETING'").expect("child");
    assert_eq!(unexported.output(), "value:\n");
    sh.eval("export GREETING").expect("export");
    let exported = sh.eval("sh -c 'echo value:$GREETING'").expect("child");
    assert_eq!(exported.output(), "value:hi\n");
    // Per-command env prefix.
    let prefixed = shell().eval("ONLY=here sh -c 'echo got:$ONLY'").expect("eval");
    assert_eq!(prefixed.output(), "got:here\n");
}

#[test]
fn cd_changes_shared_cwd() {
    let sh = shell();
    sh.eval("cd /").expect("cd");
    assert_eq!(sh.cwd(), std::path::PathBuf::from("/"));
    let report = sh.eval("sh -c pwd").expect("pwd");
    assert_eq!(report.output(), "/\n");
    let bad = sh.eval("cd /nonexistent-dir-xyz").expect("cd failure is a status");
    assert_eq!(bad.status, 1);
}

#[test]
fn exit_is_a_request_to_the_embedder() {
    assert!(matches!(
        shell().eval("exit 7"),
        Err(Error::ExitRequested { code: 7 })
    ));
}

#[test]
fn state_builtins_refuse_pipelines() {
    assert!(matches!(
        shell().eval("cd / | cat"),
        Err(Error::BuiltinInPipeline { .. })
    ));
}

#[test]
fn background_runs_share_state_and_wait_joins() {
    let sh = shell();
    let report = sh.eval("echo bg-value & wait").expect("eval");
    assert_eq!(report.background_started.len(), 1);
    let bg_id = report.background_started[0];
    let bg = sh.report(bg_id).expect("background report committed");
    assert_eq!(bg.output(), "bg-value\n");
    let reuse = sh
        .eval(&format!("echo ${{o[{bg_id}]}}"))
        .expect("background run addressable");
    assert_eq!(reuse.output(), "bg-value\n");
}

#[test]
fn detached_evals_share_state() {
    let sh = shell();
    let handle = sh.eval_detached("SHARED=from-detached");
    handle.join().expect("join");
    assert_eq!(sh.var("SHARED").as_deref(), Some("from-detached"));
    let concurrent = sh.eval_detached("sleep 0.1; echo slow");
    let fast = sh.eval("echo fast").expect("fast");
    assert_eq!(fast.output(), "fast\n");
    let slow = concurrent.join().expect("slow");
    assert_eq!(slow.output(), "slow\n");
}

#[test]
fn truncation_is_loud_and_bounded() {
    let sh = Shell::new(Config {
        capture_limit: 64,
        ..Config::default()
    })
    .expect("shell");
    let report = sh.eval("yes abcdefgh | head -n 1000").expect("eval");
    let stage = &report.pipelines[0].stages[1];
    assert_eq!(stage.stdout.total_bytes(), 9000);
    assert!(stage.stdout.truncated());
    assert!(stage.stdout.render().contains("omitted"));
}

#[test]
fn reports_serialize_to_json() {
    let report = shell().eval("echo json | cat").expect("eval");
    let json = serde_json::to_value(&report).expect("serialize");
    assert_eq!(json["pipelines"][0]["stages"][1]["stdout"]["text"], "json\n");
    assert_eq!(json["status"], 0);
}

#[test]
fn echo_dash_n() {
    let report = shell().eval("echo -n bare").expect("eval");
    assert_eq!(report.output(), "bare");
}

#[test]
fn quoting_reaches_argv_intact() {
    let report = shell()
        .eval(r#"printf '%s|' "a b" 'c d' e"#)
        .expect("eval");
    assert_eq!(report.output(), "a b|c d|e|");
}

#[test]
fn keep_runs_evicts_old_runs() {
    let sh = Shell::new(Config {
        keep_runs: 2,
        ..Config::default()
    })
    .expect("shell");
    sh.eval("echo one").expect("run 1");
    sh.eval("echo two").expect("run 2");
    sh.eval("echo three").expect("run 3");
    assert!(
        matches!(sh.eval("echo ${o[1]}"), Err(Error::RunRef { .. })),
        "run 1 evicted"
    );
    let ok = sh.eval("echo ${o[3]}").expect("run 3 retained");
    assert_eq!(ok.output(), "three\n");
    assert_eq!(sh.reports().len(), 2);
}

#[test]
fn same_eval_self_reference() {
    let report = shell()
        .eval("echo alpha | tr a-z A-Z; echo again=${o[1][0]}")
        .expect("eval");
    assert_eq!(report.output(), "again=alpha\n");
}

#[test]
fn structured_run_paths() {
    let sh = shell();
    sh.eval("echo alpha | tr a-z A-Z").expect("run 1");
    let report = sh
        .eval("echo ${runs[1].output} ${runs[1].stages[0].stdout} ${runs[1].status} ${runs[-1].stages[-1].argv}")
        .expect("paths");
    assert_eq!(report.output(), "ALPHA alpha 0 tr a-z A-Z\n");
    let bytes = sh
        .eval("echo ${runs[1].stages[0].stdout_bytes}")
        .expect("bytes");
    assert_eq!(bytes.output(), "6\n");
    for src in [
        "echo ${runs[1]}",
        "echo ${runs[1].nope}",
        "echo ${runs[1].stages[0].nope}",
        "echo ${runs[1].stages[0]}",
        "echo ${runs.output}",
        "echo ${HOME[0]}",
    ] {
        assert!(
            matches!(sh.eval(src), Err(Error::RunRef { .. })),
            "{src} should be a run-reference error"
        );
    }
}

#[test]
fn negative_run_references() {
    let sh = shell();
    sh.eval("echo newest").expect("run 1");
    let previous = sh.eval("echo prev=${o[-1]}").expect("run 2");
    assert_eq!(previous.output(), "prev=newest\n");
    sh.eval("echo hi | tr a-z A-Z").expect("run 3");
    let stage = sh.eval("echo ${o[-1][-2]}").expect("run 4");
    assert_eq!(stage.output(), "hi\n");
}

#[test]
fn echoing_config_still_captures() {
    let report = echoing().eval("echo visible").expect("eval");
    assert_eq!(report.output(), "visible\n");
}
