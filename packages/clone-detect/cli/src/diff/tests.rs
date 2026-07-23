use std::path::PathBuf;

use super::{ChangedLines, parse_unified_diff};

/// Lines recorded for `path` in a parsed diff, as a sorted vec for assertions.
fn lines_for(changed: &ChangedLines, path: &str) -> Vec<usize> {
    changed
        .0
        .get(&PathBuf::from(path))
        .map(|set| set.iter().copied().collect())
        .unwrap_or_default()
}

#[test]
fn single_modified_line() {
    // `@@ -2 +2 @@` with no count means one new-side line at 2.
    let diff = "\
diff --git a/f.txt b/f.txt
--- a/f.txt
+++ b/f.txt
@@ -2 +2 @@ a
-b
+B2
";
    let changed = parse_unified_diff(diff).unwrap();
    assert_eq!(lines_for(&changed, "f.txt"), vec![2]);
}

#[test]
fn added_block_uses_count() {
    // `@@ -3,0 +4,2 @@` adds two lines starting at 4.
    let diff = "\
+++ b/f.txt
@@ -3,0 +4,2 @@ c
+X
+Y
";
    let changed = parse_unified_diff(diff).unwrap();
    assert_eq!(lines_for(&changed, "f.txt"), vec![4, 5]);
}

#[test]
fn pure_deletion_contributes_no_lines() {
    // A deletion hunk has a new-side count of 0: `@@ -5,3 +4,0 @@`.
    let diff = "\
+++ b/f.txt
@@ -5,3 +4,0 @@ c
-gone1
-gone2
-gone3
";
    let changed = parse_unified_diff(diff).unwrap();
    assert!(lines_for(&changed, "f.txt").is_empty());
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
    assert!(changed.0.is_empty());
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
    assert!(changed.0.is_empty());
}

#[test]
fn hunk_before_any_file_header_is_ignored() {
    // Defensive: a stray hunk with no preceding `+++` has nothing to attribute.
    let diff = "@@ -1 +1 @@\n-a\n+b\n";
    let changed = parse_unified_diff(diff).unwrap();
    assert!(changed.0.is_empty());
}
