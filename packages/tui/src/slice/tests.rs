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
fn test_slice_2d_row_out_of_bounds() {
    let lines = vec!["line1".to_string(), "line2".to_string()];

    let result = slice_2d(
        &lines,
        RowRange::new(Some(1), Some(5)),
        ColRange::new(None, None),
    );

    assert!(result.is_err());
}

#[test]
fn test_slice_2d_col_out_of_bounds() {
    let lines = vec!["test".to_string()];

    let result = slice_2d(
        &lines,
        RowRange::new(None, None),
        ColRange::new(Some(1), Some(10)),
    );

    assert!(result.is_err());
}

#[test]
fn test_slice_2d_invalid_row_order() {
    let lines = vec!["line1".to_string(), "line2".to_string()];

    let result = slice_2d(
        &lines,
        RowRange::new(Some(2), Some(1)),
        ColRange::new(None, None),
    );

    assert!(result.is_err());
}

#[test]
fn test_slice_2d_invalid_col_order() {
    let lines = vec!["test".to_string()];

    let result = slice_2d(
        &lines,
        RowRange::new(None, None),
        ColRange::new(Some(3), Some(1)),
    );

    assert!(result.is_err());
}

#[test]
fn test_slice_2d_zero_row_index() {
    let lines = vec!["test".to_string()];

    let result = slice_2d(
        &lines,
        RowRange::new(Some(0), Some(1)),
        ColRange::new(None, None),
    );

    assert!(result.is_err());
}

#[test]
fn test_slice_2d_zero_col_index() {
    let lines = vec!["test".to_string()];

    let result = slice_2d(
        &lines,
        RowRange::new(None, None),
        ColRange::new(Some(0), Some(2)),
    );

    assert!(result.is_err());
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
