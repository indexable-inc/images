use super::*;

#[test]
fn test_parse_simple_conflict() {
    let content = r"before
<<<<<<< HEAD
left content
=======
right content
>>>>>>> branch
after";

    let parsed = conflicts(content);
    assert!(parsed.has_conflicts);
    assert_eq!(parsed.conflicts.len(), 1);

    let conflict = parsed.conflicts.first().unwrap();
    assert_eq!(conflict.left, "left content");
    assert_eq!(conflict.right, "right content");
    assert!(conflict.base.is_none());
    assert_eq!(conflict.left_name, "HEAD");
    assert_eq!(conflict.right_name, "branch");
}

#[test]
fn test_parse_diff3_conflict() {
    let content = r"<<<<<<< ours
left
||||||| base
original
=======
right
>>>>>>> theirs";

    let parsed = conflicts(content);
    assert!(parsed.has_conflicts);

    let conflict = parsed.conflicts.first().unwrap();
    assert_eq!(conflict.left, "left");
    assert_eq!(conflict.base.as_deref(), Some("original"));
    assert_eq!(conflict.right, "right");
}

#[test]
fn test_parse_no_conflicts() {
    let content = "just normal content\nno conflicts here";
    let parsed = conflicts(content);
    assert!(!parsed.has_conflicts);
    assert!(parsed.conflicts.is_empty());
}

#[test]
fn test_format_conflict_diff2() {
    let settings = DisplaySettings {
        marker_size: 7,
        diff3_style: false,
        left_name: String::from("ours"),
        right_name: String::from("theirs"),
        base_name: String::from("base"),
    };

    let input = format::ConflictInput {
        left: "left",
        base: Some("base"),
        right: "right",
    };
    let output = format::conflict(&input, &settings);
    assert!(output.contains("<<<<<<<"));
    assert!(output.contains("======="));
    assert!(output.contains(">>>>>>>"));
    assert!(!output.contains("|||||||"));
}

#[test]
fn test_format_conflict_diff3() {
    let settings = DisplaySettings::default();

    let input = format::ConflictInput {
        left: "left",
        base: Some("base"),
        right: "right",
    };
    let output = format::conflict(&input, &settings);
    assert!(output.contains("<<<<<<<"));
    assert!(output.contains("|||||||"));
    assert!(output.contains("======="));
    assert!(output.contains(">>>>>>>"));
}

#[test]
fn test_display_settings_default() {
    let settings = DisplaySettings::default();
    assert_eq!(settings.marker_size, 7);
    assert!(settings.diff3_style);
}

#[test]
fn test_driver_result_success() {
    let result = DriverResult::success(String::from("merged"));
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.conflict_count, 0);
}

#[test]
fn test_driver_result_with_conflicts() {
    let result = DriverResult::with_conflicts(String::from("content"), 3);
    assert_eq!(result.exit_code, 1);
    assert_eq!(result.conflict_count, 3);
}

#[test]
fn test_extract_oid_from_marker() {
    assert_eq!(
        extract_oid_from_marker("<<<<<<< HEAD:file.txt"),
        Some("HEAD")
    );
    assert_eq!(
        extract_oid_from_marker("<<<<<<< abc123:path/to/file"),
        Some("abc123")
    );
    assert_eq!(extract_oid_from_marker("not a marker"), None);
}

#[test]
fn test_parse_multiline_conflict() {
    let content = r"<<<<<<< HEAD
line 1
line 2
line 3
=======
other 1
other 2
>>>>>>> branch";

    let parsed = conflicts(content);
    let conflict = parsed.conflicts.first().unwrap();
    assert!(conflict.left.contains("line 1"));
    assert!(conflict.left.contains("line 2"));
    assert!(conflict.left.contains("line 3"));
    assert!(conflict.right.contains("other 1"));
    assert!(conflict.right.contains("other 2"));
}

#[test]
fn test_format_conflict_preserves_newlines() {
    let settings = DisplaySettings::default();
    let input = format::ConflictInput {
        left: "line1\nline2",
        base: None,
        right: "other",
    };
    let output = format::conflict(&input, &settings);

    assert!(output.lines().count() > 4);
}
