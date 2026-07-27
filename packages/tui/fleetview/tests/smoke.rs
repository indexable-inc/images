//! Drive the real binary through a PTY and read what a human would see.
//!
//! The point of the tool is that it infers state from screen activity, and
//! that inference cannot be tested from the outside without a real terminal,
//! a real child process, and real time passing. So this spawns `fleetview`
//! itself on a PTY (the same `tui` crate it uses internally), dispatches a
//! stand-in agent whose activity is scripted, and asserts the list moves the
//! session between sections as the screen goes quiet.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use tui::{SpawnConfig, TuiInstance, TuiManager};

/// Chatters for ~1.8s, then holds still forever. Long enough to be seen
/// working, quiet enough afterwards to settle into awaiting input.
const FAKE_AGENT: &str = r#"#!/bin/sh
i=0
while [ $i -lt 12 ]; do
  printf 'tick %s\r\n' "$i"
  i=$((i + 1))
  sleep 0.15
done
printf 'AGENT IDLE\r\n'
while sleep 1; do :; done
"#;

/// Generous: a loaded CI box still has to get through spawn, first paint, and
/// the settle window.
const TIMEOUT: Duration = Duration::from_secs(20);

fn write_fake_agent(dir: &Path) -> PathBuf {
    let path = dir.join("fake-agent");
    fs::write(&path, FAKE_AGENT).expect("write fake agent");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake agent");
    path
}

fn launch(dir: &Path, agent: &Path) -> (TuiManager, TuiInstance) {
    let manager = TuiManager::new();
    let instance = manager
        .spawn(
            env!("CARGO_BIN_EXE_fleetview").to_owned(),
            vec![
                "--cwd".to_owned(),
                dir.display().to_string(),
                "--command".to_owned(),
                agent.display().to_string(),
            ],
            SpawnConfig {
                rows: 30,
                cols: 100,
                ..SpawnConfig::default()
            },
        )
        .expect("spawn fleetview");
    (manager, instance)
}

fn screen(instance: &TuiInstance) -> String {
    instance.read_viewport().unwrap_or_default().join("\n")
}

/// Poll until `needle` shows up, or fail with the screen that never contained it.
fn wait_for(instance: &TuiInstance, needle: &str) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let seen = screen(instance);
        if seen.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needle:?}; screen was:\n{seen}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

fn type_task(instance: &TuiInstance, task: &str) {
    instance.write(task).expect("type task");
    wait_for(instance, task);
    instance.write("\r").expect("submit task");
}

#[test]
fn dispatches_a_session_and_follows_it_from_working_to_awaiting_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = write_fake_agent(dir.path());
    let (_manager, fleet) = launch(dir.path(), &agent);

    // The empty fleet greets you with the prompt, not a list.
    wait_for(&fleet, "describe a task for a new session");
    wait_for(&fleet, "0 awaiting input");

    type_task(&fleet, "watch the fake agent");

    // A freshly dispatched agent is painting, so it lands under `working`.
    wait_for(&fleet, "1 working");
    wait_for(&fleet, "watch the fake agent");

    // Once its screen holds still, it moves itself to `awaiting input` and the
    // row previews the last thing it said.
    wait_for(&fleet, "1 awaiting input");
    wait_for(&fleet, "AGENT IDLE");
}

/// Attach to the session under the cursor, in a list ordered newest first.
#[test]
fn ctrl_n_and_ctrl_p_pick_which_session_enter_attaches_to() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = write_fake_agent(dir.path());
    let (_manager, fleet) = launch(dir.path(), &agent);
    wait_for(&fleet, "describe a task for a new session");

    type_task(&fleet, "first task");
    type_task(&fleet, "second task");
    // Wait for both to settle before navigating: a session that changes section
    // mid-test would reorder the list under the cursor.
    wait_for(&fleet, "2 awaiting input");

    // Newest first, and dispatching selects what it dispatched, so the cursor
    // sits on "second task" and the older one is the row below.
    fleet.write("\x0e").expect("ctrl-n");
    fleet.write("\r").expect("attach");
    wait_for(&fleet, "ctrl-o");
    let attached = screen(&fleet);
    assert!(
        attached.contains("first task"),
        "ctrl-n did not move the selection down:\n{attached}"
    );
    // The agent's own screen fills the pane; the list is gone.
    assert!(
        !attached.contains("describe a task"),
        "the list is still showing under the attached session:\n{attached}"
    );
    // The pane is the live terminal, not a snapshot taken on attach.
    wait_for(&fleet, "AGENT IDLE");

    fleet.write("\x0f").expect("ctrl-o");
    wait_for(&fleet, "describe a task for a new session");

    // And ctrl-p walks back up to the newer one.
    fleet.write("\x10").expect("ctrl-p");
    fleet.write("\r").expect("attach");
    wait_for(&fleet, "ctrl-o");
    let attached = screen(&fleet);
    assert!(
        attached.contains("second task"),
        "ctrl-p did not move the selection up:\n{attached}"
    );
}

#[test]
fn a_stopped_session_is_reported_as_completed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = write_fake_agent(dir.path());
    let (_manager, fleet) = launch(dir.path(), &agent);
    wait_for(&fleet, "describe a task for a new session");

    type_task(&fleet, "doomed task");
    wait_for(&fleet, "doomed task");

    fleet.write("\x18").expect("ctrl-x");
    wait_for(&fleet, "1 completed");
}
