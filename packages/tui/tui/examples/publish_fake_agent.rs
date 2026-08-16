//! Publish one fake agent to the dashboard: a PTY that paints a busy footer
//! then goes quiet, beside a hand-written Claude-shaped session log growing a
//! line at a time. The smallest live fixture for the transcript pane and the
//! working -> awaiting_input status flip, with no real agent run's latency.
//!
//! Run the aggregator (`cargo run -p dashboard`), then:
//!
//! ```sh
//! cargo run -p tui --example publish_fake_agent --features dashboard,publish
//! ```
// Auto-discovered, deliberately absent from the manifest, exactly like
// publish_cat: the nix cargo-unit planner builds from a source set without
// `examples/`, so a `[[example]]` entry fails its manifest parse.
#[cfg(all(feature = "dashboard", feature = "publish"))]
fn main() {
    use std::io::Write as _;
    use std::sync::Arc;
    use std::time::Duration;

    let pid = std::process::id();
    let log_root = std::env::temp_dir().join(format!("ix-fake-claude-{pid}"));
    let cwd = std::env::temp_dir().join(format!("ix-fake-agent-cwd-{pid}"));

    let manager = Arc::new(tui::TuiManager::new());
    let mut config = tui::SpawnConfig::default();
    config.agent = Some(tui::AgentConfig {
        kind: "claude".to_owned(),
        busy_marker: Some("esc to interrupt".to_owned()),
        gate_markers: Vec::new(),
        session_log: Some(tui::SessionLogKind::Claude),
        cwd: Some(cwd.clone()),
        log_root: Some(log_root.clone()),
    });
    // Busy footer for ~4s, then a cleared screen: the marker must leave the
    // viewport (as the real footer does) for working to flip to
    // awaiting_input, so the quiet phase repaints from home.
    let script = "printf 'thinking hard (esc to interrupt)'; sleep 4; \
                  printf '\\033[2J\\033[Hdone thinking\\n'; sleep 600";
    let term = manager
        .spawn("sh".into(), vec!["-c".into(), script.into()], config)
        .expect("spawn fake agent");

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let publisher = runtime
        .block_on(tui::publish(
            &manager,
            tui::socket_path(),
            Duration::from_millis(50),
        ))
        .expect("publish");
    println!("PRODUCER={}", publisher.producer_id());
    println!("PANE={}", term.id);
    std::io::stdout().flush().expect("flush");

    // The session log, created after the spawn (as the real CLI does) and
    // grown one complete line at a time.
    let project = log_root.join("projects").join(munge(&cwd));
    std::fs::create_dir_all(&project).expect("mkdir project");
    let file = project.join("11111111-2222-3333-4444-555555555555.jsonl");
    let lines = [
        r#"{"type":"user","timestamp":"2026-08-05T00:00:01Z","message":{"role":"user","content":"FAKE-PROMPT-ONE"}}"#,
        r#"{"type":"assistant","timestamp":"2026-08-05T00:00:02Z","message":{"role":"assistant","usage":{"input_tokens":10,"output_tokens":20},"content":[{"type":"text","text":"FAKE-REPLY-ONE"}]}}"#,
        r#"{"type":"assistant","timestamp":"2026-08-05T00:00:03Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"FakeTool","input":{}}]}}"#,
        r#"{"type":"assistant","timestamp":"2026-08-05T00:00:04Z","message":{"role":"assistant","usage":{"input_tokens":30,"output_tokens":40},"content":[{"type":"text","text":"FAKE-REPLY-TWO"}]}}"#,
    ];
    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .expect("open log");
    for line in lines {
        writeln!(handle, "{line}").expect("append");
        handle.flush().expect("flush");
        std::thread::sleep(Duration::from_millis(1500));
    }
    println!("LOG-DONE");
    std::io::stdout().flush().expect("flush");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[cfg(all(feature = "dashboard", feature = "publish"))]
fn munge(cwd: &std::path::Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(not(all(feature = "dashboard", feature = "publish")))]
fn main() {
    eprintln!("publish_fake_agent needs --features dashboard,publish");
}
