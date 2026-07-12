use super::{ColRange, RowRange, slice_2d};

#[test]
fn test_slice_2d_full_range() {
    let lines = vec![
        "hello world".to_string(),
        "foo bar".to_string(),
        "test".to_string(),
    ];

    let result = slice_2d(&lines, RowRange::new(None, None), ColRange::new(None, None)).unwrap();

    assert_eq!(result, lines);
}

#[test]
fn test_slice_2d_row_subset() {
    let lines = vec![
        "line1".to_string(),
        "line2".to_string(),
        "line3".to_string(),
        "line4".to_string(),
    ];

    let result = slice_2d(
        &lines,
        RowRange::new(Some(2), Some(3)),
        ColRange::new(None, None),
    )
    .unwrap();

    assert_eq!(result, vec!["line2", "line3"]);
}

#[test]
fn test_slice_2d_col_subset() {
    let lines = vec!["hello world".to_string(), "foo bar baz".to_string()];

    let result = slice_2d(
        &lines,
        RowRange::new(None, None),
        ColRange::new(Some(1), Some(5)),
    )
    .unwrap();

    assert_eq!(result, vec!["hello", "foo b"]);
}

#[test]
fn test_slice_2d_both_ranges() {
    let lines = vec![
        "abcdefgh".to_string(),
        "ijklmnop".to_string(),
        "qrstuvwx".to_string(),
    ];

    let result = slice_2d(
        &lines,
        RowRange::new(Some(1), Some(2)),
        ColRange::new(Some(2), Some(5)),
    )
    .unwrap();

    assert_eq!(result, vec!["bcde", "jklm"]);
}

#[test]
fn test_slice_2d_single_row() {
    let lines = vec!["test".to_string(), "data".to_string()];

    let result = slice_2d(
        &lines,
        RowRange::new(Some(2), Some(2)),
        ColRange::new(None, None),
    )
    .unwrap();

    assert_eq!(result, vec!["data"]);
}

#[test]
fn test_slice_2d_empty_lines() {
    let lines: Vec<String> = vec![];

    let result = slice_2d(&lines, RowRange::new(None, None), ColRange::new(None, None)).unwrap();

    assert!(result.is_empty());
}

#[test]
fn test_slice_2d_rejects_invalid_ranges() {
    let lines = ["line1".to_string(), "line2".to_string()];
    let cases = [
        (
            "row out of bounds",
            RowRange::new(Some(1), Some(5)),
            ColRange::new(None, None),
        ),
        (
            "column out of bounds",
            RowRange::new(None, None),
            ColRange::new(Some(1), Some(10)),
        ),
        (
            "reversed rows",
            RowRange::new(Some(2), Some(1)),
            ColRange::new(None, None),
        ),
        (
            "reversed columns",
            RowRange::new(None, None),
            ColRange::new(Some(3), Some(1)),
        ),
        (
            "zero row",
            RowRange::new(Some(0), Some(1)),
            ColRange::new(None, None),
        ),
        (
            "zero column",
            RowRange::new(None, None),
            ColRange::new(Some(0), Some(2)),
        ),
    ];

    for (case, rows, columns) in cases {
        assert!(slice_2d(&lines, rows, columns).is_err(), "{case}");
    }
}

#[test]
fn test_slice_2d_empty_line() {
    let lines = vec![String::new(), "data".to_string()];

    let result = slice_2d(&lines, RowRange::new(None, None), ColRange::new(None, None)).unwrap();

    assert_eq!(result, vec!["", "data"]);
}

#[test]
fn test_slice_2d_unicode() {
    let lines = vec!["hello 世界".to_string(), "foo 🦀 bar".to_string()];

    let result = slice_2d(
        &lines,
        RowRange::new(None, None),
        ColRange::new(Some(7), Some(8)),
    )
    .unwrap();

    assert_eq!(result, vec!["世界", "ba"]);
}
