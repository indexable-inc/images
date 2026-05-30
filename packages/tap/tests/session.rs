//! End-to-end tests that drive the real `tap` binary on a PTY and assert on its
//! rendered screen, reusing the workspace's `tui` PTY driver as the harness.
//!
//! Each test starts a session with `tap start`, attaches more clients with `tap
//! attach`, types into them, and reads back the vt100 grid the client paints.
//! This is the layer the original tap broke at: attaching to a running
//! full-screen TUI, resizing while attached, and sharing a session. The four
//! tests below pin exactly those behaviors.
//!
//! State is isolated under a per-process `TAP_RUNTIME_DIR`; every test uses a
//! distinct session id and tears its session down with a [`SessionGuard`] so a
//! detached daemon never outlives the test.

use std::sync::Once;
use std::time::{Duration, Instant};

use tui::{SpawnConfig, TuiInstance, TuiManager};

/// The env var tap reads for its runtime dir (kept in sync with `tap-protocol`).
const RUNTIME_DIR_ENV: &str = "TAP_RUNTIME_DIR";

/// Path to the `tap` binary under test (set by cargo for integration tests).
fn tap_bin() -> String {
    env!("CARGO_BIN_EXE_tap").to_string()
}

/// Point tap at an isolated runtime dir once, before any session is spawned.
fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("tap-it-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create runtime dir");
        // Single-threaded init; inherited by every spawned tap/daemon process.
        unsafe { std::env::set_var(RUNTIME_DIR_ENV, dir) };
    });
}

const fn config(rows: u16, cols: u16) -> SpawnConfig {
    SpawnConfig {
        rows,
        cols,
        scrollback_lines: 1000,
    }
}

fn session_id(prefix: &str) -> String {
    format!("it-{prefix}-{}", std::process::id())
}

/// Kills the session's daemon on drop so a detached daemon never leaks.
struct SessionGuard {
    id: String,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let _ = std::process::Command::new(tap_bin())
            .args(["kill", &self.id])
            .output();
    }
}

/// Poll a client's rendered viewport until `needle` appears or time runs out.
fn wait_for(client: &TuiInstance, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(rows) = client.read_viewport()
            && rows.iter().any(|row| row.contains(needle))
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run `tap <args>` and return its stdout.
fn tap_command(args: &[&str]) -> String {
    let output = std::process::Command::new(tap_bin())
        .args(args)
        .output()
        .expect("run tap subcommand");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Poll `tap <args>` until its stdout contains `needle` or time runs out. The
/// per-attempt connection can transiently fail under heavy parallel load, so
/// queries are retried rather than asserted one-shot.
fn wait_for_command(args: &[&str], needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if tap_command(args).contains(needle) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn session_round_trips_input_and_serves_scrollback() {
    init();
    let id = session_id("roundtrip");
    let _guard = SessionGuard { id: id.clone() };
    let manager = TuiManager::new();

    let client = manager
        .spawn(
            tap_bin(),
            vec!["start".into(), "--id".into(), id.clone(), "bash".into()],
            config(24, 80),
        )
        .expect("start session");

    client.write("echo round-trip-OK\n").expect("type command");
    assert!(
        wait_for(&client, "round-trip-OK", Duration::from_secs(10)),
        "attached client never showed the command output"
    );

    // A separate process can read the same session's screen over the socket.
    assert!(
        wait_for_command(
            &["scrollback", "--session", &id],
            "round-trip-OK",
            Duration::from_secs(10),
        ),
        "scrollback query never returned the session output"
    );
}

#[test]
fn second_attach_resyncs_a_full_screen_tui() {
    init();
    let id = session_id("altscreen");
    let _guard = SessionGuard { id: id.clone() };
    let manager = TuiManager::new();

    // Enter the alternate screen, draw colored text, then block so the session
    // stays in its full-screen state while a second client attaches.
    let script = "printf '\\033[?1049h\\033[2J\\033[H\\033[31mRED-TUI-BODY\\033[0m'; read ignored";
    let first = manager
        .spawn(
            tap_bin(),
            vec![
                "start".into(),
                "--id".into(),
                id.clone(),
                "bash".into(),
                "-c".into(),
                script.into(),
            ],
            config(24, 80),
        )
        .expect("start full-screen session");
    assert!(
        wait_for(&first, "RED-TUI-BODY", Duration::from_secs(10)),
        "first client never rendered the full-screen body"
    );

    // The original tap dumped colorless plain text with no redraw here; the
    // resync snapshot must reproduce the alternate-screen content instead.
    let second = manager
        .spawn(tap_bin(), vec!["attach".into(), id], config(24, 80))
        .expect("attach second client");
    assert!(
        wait_for(&second, "RED-TUI-BODY", Duration::from_secs(10)),
        "second client did not resync the full-screen content on attach"
    );
}

#[test]
fn multiplayer_shares_output_and_sizes_to_smallest_client() {
    init();
    let id = session_id("multiplayer");
    let _guard = SessionGuard { id: id.clone() };
    let manager = TuiManager::new();

    let small = manager
        .spawn(
            tap_bin(),
            vec!["start".into(), "--id".into(), id.clone(), "bash".into()],
            config(24, 80),
        )
        .expect("start session");
    // Let the shell come up before a second client joins.
    std::thread::sleep(Duration::from_millis(400));

    let large = manager
        .spawn(
            tap_bin(),
            vec!["attach".into(), id.clone()],
            config(30, 100),
        )
        .expect("attach larger client");

    // The larger client is told the session is sized to the smaller one, and
    // warns that its extra space is unused.
    assert!(
        wait_for(&large, "smallest client wins", Duration::from_secs(10)),
        "larger client showed no size-mismatch warning"
    );

    // Output typed in one client reaches both.
    small.write("echo SHARED-OUTPUT\n").expect("type in small client");
    assert!(
        wait_for(&small, "SHARED-OUTPUT", Duration::from_secs(10)),
        "originating client missing shared output"
    );
    assert!(
        wait_for(&large, "SHARED-OUTPUT", Duration::from_secs(10)),
        "second client missing shared output"
    );

    // The negotiated size is the element-wise min of both clients.
    assert!(
        wait_for_command(&["size", "--session", &id], "24x80", Duration::from_secs(10)),
        "session not sized to the smallest client; got {:?}",
        tap_command(&["size", "--session", &id])
    );
}

#[test]
fn resize_while_attached_reaches_the_session() {
    init();
    let id = session_id("resize");
    let _guard = SessionGuard { id: id.clone() };
    let manager = TuiManager::new();

    let client = manager
        .spawn(
            tap_bin(),
            vec!["start".into(), "--id".into(), id.clone(), "bash".into()],
            config(24, 80),
        )
        .expect("start session");
    std::thread::sleep(Duration::from_millis(400));

    // Resizing the client's terminal delivers SIGWINCH to the attached tap,
    // which must forward the new size to the daemon (the original tap dropped
    // resizes from an attached client entirely).
    client.resize(40, 100).expect("resize client terminal");
    assert!(
        wait_for_command(&["size", "--session", &id], "40x100", Duration::from_secs(10)),
        "resize while attached never reached the session; got {:?}",
        tap_command(&["size", "--session", &id])
    );
}
