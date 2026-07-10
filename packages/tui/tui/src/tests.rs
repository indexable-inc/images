use std::time::Duration;

use crate::{SpawnConfig, TuiManager};

fn spawn(manager: &TuiManager, command: &str, args: &[&str]) -> crate::TuiInstance {
    let args = args.iter().map(|a| (*a).to_string()).collect();
    manager
        .spawn(command.to_string(), args, SpawnConfig::default())
        .expect("spawn failed")
}

#[test]
fn spawn_and_list() {
    let manager = TuiManager::new();
    let instance = spawn(&manager, "echo", &["test"]);

    let list = manager.list();
    assert_eq!(list.len(), 1);
    let first = list.first().expect("list should have one item");
    assert_eq!(first.id, instance.id);
    assert_eq!(first.command, "echo");
}

#[test]
fn write_and_read() {
    let manager = TuiManager::new();
    let instance = spawn(&manager, "cat", &[]);

    instance.write("hello\n").expect("write failed");

    std::thread::sleep(Duration::from_millis(100));

    let output = instance
        .read_blocking(Duration::from_secs(1))
        .expect("read failed");

    assert!(!output.is_empty());
    let first = output
        .first()
        .expect("output should have at least one line");
    assert_eq!(first, "hello");
}

#[test]
fn vim_spawns_and_produces_output() {
    let manager = TuiManager::new();
    let instance = spawn(&manager, "vim", &["-u", "NONE"]);

    let list = manager.list();
    assert_eq!(list.len(), 1);
    let first = list.first().expect("should have vim instance");
    assert_eq!(first.command, "vim");

    std::thread::sleep(Duration::from_secs(1));

    let output = instance.read_viewport();
    assert!(
        output.is_ok(),
        "vim should eventually have screen content available"
    );
}

#[test]
fn vim_help_command_changes_screen() {
    let manager = TuiManager::new();
    let instance = spawn(&manager, "vim", &["-u", "NONE"]);

    std::thread::sleep(Duration::from_secs(1));

    let initial_output = instance
        .read_viewport()
        .expect("vim should have initial screen content");

    instance
        .write(":help\n")
        .expect("failed to send help command to vim");

    std::thread::sleep(Duration::from_millis(500));

    let help_output = instance
        .read_viewport()
        .expect("vim should have screen content after help command");

    assert_ne!(
        initial_output, help_output,
        "vim screen should change after opening help"
    );
}

#[test]
fn scrollback_limit_is_configurable() {
    let manager = TuiManager::new();
    let instance = manager
        .spawn(
            "echo".to_string(),
            vec!["test".to_string()],
            SpawnConfig {
                scrollback_lines: 5000,
                ..SpawnConfig::default()
            },
        )
        .expect("spawn failed with scrollback");

    assert_eq!(instance.scrollback_limit, 5000);
}

#[test]
fn spawn_config_sets_terminal_size() {
    let manager = TuiManager::new();
    let instance = manager
        .spawn(
            "cat".to_string(),
            vec![],
            SpawnConfig {
                rows: 40,
                cols: 120,
                ..SpawnConfig::default()
            },
        )
        .expect("spawn failed");

    assert_eq!((instance.rows(), instance.cols()), (40, 120));
}

#[test]
fn spawn_config_env_reaches_child_and_forced_term_wins() {
    let manager = TuiManager::new();
    let instance = manager
        .spawn(
            "sh".to_string(),
            vec!["-c".to_string(), "echo \"$IX_TEST_VAR:$TERM\"".to_string()],
            SpawnConfig {
                // A caller TERM must lose to the crate-forced xterm-256color.
                env: vec![
                    ("IX_TEST_VAR".to_string(), "from-config".to_string()),
                    ("TERM".to_string(), "dumb".to_string()),
                ],
                ..SpawnConfig::default()
            },
        )
        .expect("spawn failed with env");

    std::thread::sleep(Duration::from_millis(300));

    let viewport = instance.read_viewport().expect("viewport read failed");
    assert!(
        viewport
            .iter()
            .any(|line| line.contains("from-config:xterm-256color")),
        "caller env should reach the child and forced TERM should win, got {viewport:?}"
    );
}

#[test]
fn resize_changes_reported_size_and_emulator_width() {
    let manager = TuiManager::new();
    let instance = spawn(&manager, "cat", &[]);

    instance.resize(30, 100).expect("resize failed");
    assert_eq!((instance.rows(), instance.cols()), (30, 100));

    // The clone shares the size, and a wide line round-trips at the new width.
    let same = manager.get(&instance.id).expect("instance present");
    assert_eq!((same.rows(), same.cols()), (30, 100));

    instance
        .write(&format!("{}\n", "x".repeat(90)))
        .expect("write failed");
    std::thread::sleep(Duration::from_millis(100));
    let output = instance
        .read_blocking(Duration::from_secs(1))
        .expect("read failed");
    assert!(
        output.iter().any(|line| line.contains(&"x".repeat(90))),
        "a 90-wide line should fit on a 100-column screen without wrapping"
    );
}

#[test]
fn burst_output_within_viewport() {
    let manager = TuiManager::new();
    let instance = spawn(
        &manager,
        "sh",
        &["-c", "for i in 1 2 3 4 5; do echo line$i; done"],
    );

    std::thread::sleep(Duration::from_millis(200));

    let output = instance.read_viewport().expect("read failed");

    assert!(
        output.iter().any(|line| line.contains("line1")),
        "should contain line1"
    );
    assert!(
        output.iter().any(|line| line.contains("line5")),
        "should contain line5"
    );
}

#[test]
fn scrollback_captures_lines_beyond_viewport() {
    let manager = TuiManager::new();
    let instance = spawn(&manager, "sh", &["-c", "seq 0 100"]);

    std::thread::sleep(Duration::from_millis(300));

    let viewport = instance.read_viewport().expect("viewport read failed");
    let scrollback = instance.read_scrollback().expect("scrollback read failed");

    assert!(
        viewport.iter().any(|line| line.contains("100")),
        "viewport should contain line 100"
    );

    assert!(
        !scrollback.is_empty(),
        "scrollback should not be empty when output exceeds viewport height"
    );

    assert!(
        scrollback.iter().any(|line| line.contains('0')),
        "scrollback should contain early lines that scrolled off viewport"
    );

    let first_line = scrollback
        .first()
        .expect("scrollback should have at least one line");
    assert!(
        first_line.trim().starts_with('0') || first_line.trim().starts_with('1'),
        "scrollback first line should be 0 or 1, got: {first_line}"
    );

    for line in &viewport {
        assert!(
            !scrollback.iter().any(|sb_line| sb_line == line),
            "viewport line should not appear in scrollback: {line}"
        );
    }

    let mut all_lines = scrollback.clone();
    all_lines.extend(viewport);

    for i in 0..=100 {
        let i_str = i.to_string();
        let found_count = all_lines
            .iter()
            .filter(|line| line.split_whitespace().any(|word| word == i_str.as_str()))
            .count();
        assert_eq!(
            found_count, 1,
            "Number {i} should appear exactly once, found {found_count} times. All lines: {all_lines:?}"
        );
    }
}
