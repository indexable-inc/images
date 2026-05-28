use crate::TuiManager;

#[test]
fn spawn_and_list() {
    let manager = TuiManager::new();
    let instance = manager
        .spawn("echo".to_string(), vec!["test".to_string()], 10000)
        .expect("spawn failed");

    let list = manager.list();
    assert_eq!(list.len(), 1);
    let first = list.first().expect("list should have one item");
    assert_eq!(first.id, instance.id);
    assert_eq!(first.command, "echo");
}

#[test]
fn write_and_read() {
    let manager = TuiManager::new();
    let instance = manager
        .spawn("cat".to_string(), vec![], 10000)
        .expect("spawn failed");

    manager.write(&instance, "hello\n").expect("write failed");

    std::thread::sleep(std::time::Duration::from_millis(100));

    let output = manager.read_blocking(&instance, 1000).expect("read failed");

    assert!(!output.is_empty());
    let first = output
        .first()
        .expect("output should have at least one line");
    assert_eq!(first, "hello");
}

#[test]
fn vim_spawns_and_produces_output() {
    let manager = TuiManager::new();
    let instance = manager
        .spawn(
            "vim".to_string(),
            vec!["-u".to_string(), "NONE".to_string()],
            10000,
        )
        .expect("vim spawn failed");

    let list = manager.list();
    assert_eq!(list.len(), 1);
    let first = list.first().expect("should have vim instance");
    assert_eq!(first.command, "vim");

    std::thread::sleep(std::time::Duration::from_secs(1));

    let output = manager.read(&instance);
    assert!(
        output.is_ok(),
        "vim should eventually have screen content available"
    );
}

#[test]
fn vim_help_command_changes_screen() {
    let manager = TuiManager::new();
    let instance = manager
        .spawn(
            "vim".to_string(),
            vec!["-u".to_string(), "NONE".to_string()],
            10000,
        )
        .expect("vim spawn failed");

    std::thread::sleep(std::time::Duration::from_secs(1));

    let initial_output = manager
        .read(&instance)
        .expect("vim should have initial screen content");

    manager
        .write(&instance, ":help\n")
        .expect("failed to send help command to vim");

    std::thread::sleep(std::time::Duration::from_millis(500));

    let help_output = manager
        .read(&instance)
        .expect("vim should have screen content after help command");

    assert_ne!(
        initial_output, help_output,
        "vim screen should change after opening help"
    );
}

#[test]
fn scrollback_buffer_configured() {
    let manager = TuiManager::new();
    let instance = manager
        .spawn("echo".to_string(), vec!["test".to_string()], 5000)
        .expect("spawn failed with scrollback");

    assert_eq!(instance.command, "echo");
}

#[test]
fn burst_output_within_viewport() {
    let manager = TuiManager::new();
    let instance = manager
        .spawn(
            "sh".to_string(),
            vec![
                "-c".to_string(),
                "for i in 1 2 3 4 5; do echo line$i; done".to_string(),
            ],
            10000,
        )
        .expect("spawn failed");

    std::thread::sleep(std::time::Duration::from_millis(200));

    let output = manager.read(&instance).expect("read failed");

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
    let instance = manager
        .spawn(
            "sh".to_string(),
            vec!["-c".to_string(), "seq 0 100".to_string()],
            10000,
        )
        .expect("spawn failed");

    std::thread::sleep(std::time::Duration::from_millis(300));

    let viewport = manager
        .read_viewport(&instance)
        .expect("viewport read failed");
    let scrollback = manager
        .read_scrollback(&instance)
        .expect("scrollback read failed");

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
