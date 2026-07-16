use std::path::PathBuf;

use super::{ChangedLines, HunkOrigin, parse_unified_diff};

/// Lines recorded for `path` in a parsed diff, as a sorted vec for assertions.
fn lines_for(changed: &ChangedLines, path: &str) -> Vec<usize> {
    changed
        .lines
        .get(&PathBuf::from(path))
        .map(|set| set.iter().copied().collect())
        .unwrap_or_default()
}

/// Hunk origins recorded for `path` in a parsed diff.
fn origins_for(changed: &ChangedLines, path: &str) -> Vec<HunkOrigin> {
    changed
        .origins
        .get(&PathBuf::from(path))
        .cloned()
        .unwrap_or_default()
}

/// The expected origin of a single `f.txt` hunk: new-side range
/// `(new_start, new_count)` replacing old-side range `(old_start, old_count)`.
fn origin(
    (new_start, new_count): (usize, usize),
    (old_start, old_count): (usize, usize),
) -> HunkOrigin {
    HunkOrigin {
        new_start,
        new_count,
        old_path: PathBuf::from("f.txt"),
        old_start,
        old_count,
    }
}

/// Hunk parsing over a single `f.txt` diff: each case is (unified diff,
/// expected new-side lines, expected hunk origins).
#[test]
fn parses_new_side_lines_and_hunk_origins() {
    let cases: &[(&str, &[usize], &[HunkOrigin])] = &[
        // `@@ -2 +2 @@` with no count means one new-side line at 2.
        (
            "diff --git a/f.txt b/f.txt\n--- a/f.txt\n+++ b/f.txt\n@@ -2 +2 @@ a\n-b\n+B2\n",
            &[2],
            &[origin((2, 1), (2, 1))],
        ),
        // `@@ -3,0 +4,2 @@` adds two lines starting at 4: a pure insertion,
        // so the recorded origin has an empty old side.
        (
            "--- a/f.txt\n+++ b/f.txt\n@@ -3,0 +4,2 @@ c\n+X\n+Y\n",
            &[4, 5],
            &[origin((4, 2), (3, 0))],
        ),
        // A deletion hunk (`@@ -5,3 +4,0 @@`) has a new-side count of 0: it
        // contributes no lines and no origin.
        (
            "+++ b/f.txt\n@@ -5,3 +4,0 @@ c\n-gone1\n-gone2\n-gone3\n",
            &[],
            &[],
        ),
    ];
    for (diff, lines, origins) in cases {
        let changed = parse_unified_diff(diff).unwrap();
        assert_eq!(lines_for(&changed, "f.txt"), *lines, "diff: {diff}");
        assert_eq!(origins_for(&changed, "f.txt"), *origins, "diff: {diff}");
    }
}

#[test]
fn deleted_file_new_side_is_dev_null() {
    // `+++ /dev/null` (whole-file deletion): no new-side path to attribute to,
    // so the hunk is dropped rather than misattributed.
    let diff = "\
diff --git a/gone.txt b/gone.txt
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-a
-b
";
    let changed = parse_unified_diff(diff).unwrap();
    assert!(changed.lines.is_empty());
    assert!(changed.origins.is_empty());
}

#[test]
fn multiple_files_and_hunks() {
    let diff = "\
+++ b/a.rs
@@ -10 +10 @@
-old
+new
@@ -20,0 +21,3 @@
+p
+q
+r
+++ b/b.rs
@@ -1,0 +1,1 @@
+only
";
    let changed = parse_unified_diff(diff).unwrap();
    assert_eq!(lines_for(&changed, "a.rs"), vec![10, 21, 22, 23]);
    assert_eq!(lines_for(&changed, "b.rs"), vec![1]);
}

#[test]
fn new_file_added() {
    // A brand-new file: old side is /dev/null, new side names the file, and the
    // whole body is added.
    let diff = "\
--- /dev/null
+++ b/new.rs
@@ -0,0 +1,3 @@
+line1
+line2
+line3
";
    let changed = parse_unified_diff(diff).unwrap();
    assert_eq!(lines_for(&changed, "new.rs"), vec![1, 2, 3]);
    // No old side, no ancestry: the new file replaced nothing at the base.
    assert!(origins_for(&changed, "new.rs").is_empty());
}

#[test]
fn strips_b_prefix_only() {
    // The `b/` prefix is git's convention; a path that itself starts with a
    // literal `b/` segment after stripping is preserved.
    let diff = "\
+++ b/crate/src/main.rs
@@ -1 +1 @@
-a
+b
";
    let changed = parse_unified_diff(diff).unwrap();
    assert_eq!(lines_for(&changed, "crate/src/main.rs"), vec![1]);
}

#[test]
fn empty_diff_is_empty() {
    let changed = parse_unified_diff("").unwrap();
    assert!(changed.lines.is_empty());
    assert!(changed.origins.is_empty());
}

#[test]
fn hunk_before_any_file_header_is_ignored() {
    // Defensive: a stray hunk with no preceding `+++` has nothing to attribute.
    let diff = "@@ -1 +1 @@\n-a\n+b\n";
    let changed = parse_unified_diff(diff).unwrap();
    assert!(changed.lines.is_empty());
    assert!(changed.origins.is_empty());
}
