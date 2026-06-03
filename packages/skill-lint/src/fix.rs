//! Pure safe autofixes for SKILL.md. Each fix is conservative: it never tries
//! to repair malformed YAML (that is reported as a lint error instead), and it
//! never rewrites a file whose frontmatter fails to parse.

use std::path::Path;

/// What a fix pass produced for one file.
pub struct FixOutcome {
    /// New file contents, or `None` when the file must be left untouched
    /// (frontmatter missing or unparseable).
    pub contents: Option<String>,
    /// Human-readable descriptions of each change applied.
    pub changes: Vec<String>,
}

/// Apply safe autofixes to a SKILL.md's contents. Pure: no IO.
pub fn fix_skill(path: &Path, contents: &str) -> FixOutcome {
    // Refuse to touch a file with missing or unparseable frontmatter; surface
    // it through the linter instead so we never corrupt a broken document.
    let Some((yaml, yaml_is_mapping)) = parse_frontmatter(contents) else {
        return FixOutcome {
            contents: None,
            changes: Vec::new(),
        };
    };
    if !yaml_is_mapping {
        return FixOutcome {
            contents: None,
            changes: Vec::new(),
        };
    }

    let mut changes = Vec::new();
    let mut result = contents.to_owned();

    if !yaml.contains_key("name")
        && let Some(dir) = parent_dir_name(path)
    {
        result = insert_name(&result, dir);
        changes.push(format!("inserted `name: {dir}`"));
    }

    let normalized = normalize_whitespace(&result);
    if normalized != result {
        changes.push("stripped trailing whitespace / fixed trailing newline".to_owned());
        result = normalized;
    }

    FixOutcome {
        contents: (result != contents).then_some(result),
        changes,
    }
}

/// Parse the frontmatter block; returns the mapping (for key lookups) and
/// whether it was a mapping. `None` means no usable frontmatter. Reuses the
/// linter's splitter so fix and lint agree on what frontmatter is.
fn parse_frontmatter(contents: &str) -> Option<(serde_norway::Mapping, bool)> {
    let frontmatter = crate::lint::split_frontmatter(contents)?;
    let value: serde_norway::Value = serde_norway::from_str(frontmatter.yaml).ok()?;
    match value {
        serde_norway::Value::Mapping(mapping) => Some((mapping, true)),
        _ => Some((serde_norway::Mapping::new(), false)),
    }
}

/// Insert a `name:` line as the first frontmatter field (right after the
/// opening `---`).
fn insert_name(contents: &str, dir: &str) -> String {
    match contents.split_once('\n') {
        Some((first, rest)) => format!("{first}\nname: {dir}\n{rest}"),
        None => contents.to_owned(),
    }
}

/// Strip trailing whitespace from every line, drop trailing blank lines, and
/// ensure the file ends with exactly one newline. Preserves the file's existing
/// dominant line ending so a CRLF-authored file is not silently rewritten to LF
/// (a content-preservation violation); `str::lines()` drops `\r`, so we rejoin
/// with the detected newline instead of a hardcoded `\n`.
fn normalize_whitespace(contents: &str) -> String {
    let newline = if contents.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let mut lines: Vec<&str> = contents.lines().map(str::trim_end).collect();
    // Collapse a run of trailing blank lines so EOF carries a single newline.
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut out = lines.join(newline);
    out.push_str(newline);
    out
}

/// Helper mirroring the linter's directory-name lookup.
fn parent_dir_name(path: &Path) -> Option<&str> {
    path.parent()?.file_name()?.to_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lint::{Severity, lint_skill};

    fn skill_path() -> std::path::PathBuf {
        std::path::PathBuf::from("/repo/skills/example/SKILL.md")
    }

    #[test]
    fn inserts_missing_name_and_relint_is_clean() {
        let contents = "---\ndescription: A description.\n---\nBody.\n";
        let outcome = fix_skill(&skill_path(), contents);
        let fixed = outcome.contents.expect("fix should rewrite the file");
        assert!(fixed.contains("name: example"), "fixed:\n{fixed}");

        let diagnostics = lint_skill(&skill_path(), &fixed);
        assert!(
            !diagnostics.iter().any(|d| d.severity == Severity::Error),
            "re-lint should be clean, got {diagnostics:?}"
        );
    }

    #[test]
    fn strips_trailing_whitespace_and_fixes_newline() {
        let contents = "---\nname: example\ndescription: A description.\n---\nBody.   \n\n\n";
        let outcome = fix_skill(&skill_path(), contents);
        let fixed = outcome.contents.expect("fix should rewrite the file");
        assert!(fixed.ends_with("Body.\n"), "fixed:\n{fixed:?}");
        assert!(!fixed.contains("Body.   "), "fixed:\n{fixed:?}");
    }

    #[test]
    fn refuses_to_touch_malformed_yaml() {
        let contents =
            "---\ndescription: it does not block: it launches more work\n---\nBody.\n";
        let outcome = fix_skill(&skill_path(), contents);
        assert!(outcome.contents.is_none(), "must not rewrite broken YAML");
        assert!(outcome.changes.is_empty());
    }

    #[test]
    fn clean_file_is_left_unchanged() {
        let contents = "---\nname: example\ndescription: A description.\n---\nBody.\n";
        let outcome = fix_skill(&skill_path(), contents);
        assert!(outcome.contents.is_none(), "no change expected");
    }

    /// A clean CRLF-authored file must come back untouched: `str::lines()` strips
    /// `\r`, so before the CRLF-preserving fix this silently rewrote the file to
    /// LF. This is the regression test for that content-mutation bug.
    #[test]
    fn clean_crlf_file_is_left_unchanged() {
        let contents = "---\r\nname: example\r\ndescription: A description.\r\n---\r\nBody.\r\n";
        let outcome = fix_skill(&skill_path(), contents);
        assert!(
            outcome.contents.is_none(),
            "clean CRLF file must not be rewritten, got {:?}",
            outcome.contents
        );
        assert!(outcome.changes.is_empty());
    }

    /// A CRLF file that genuinely needs a fix keeps its CRLF endings rather than
    /// being downgraded to LF.
    #[test]
    fn crlf_file_needing_fix_keeps_crlf() {
        let contents =
            "---\r\nname: example\r\ndescription: A description.\r\n---\r\nBody.   \r\n\r\n\r\n";
        let outcome = fix_skill(&skill_path(), contents);
        let fixed = outcome.contents.expect("fix should rewrite the file");
        assert!(fixed.contains("\r\n"), "must keep CRLF, got {fixed:?}");
        assert!(fixed.ends_with("Body.\r\n"), "fixed:\n{fixed:?}");
        assert!(!fixed.contains("Body.   "), "fixed:\n{fixed:?}");
    }

    /// `fix` is idempotent: applying it to its own output yields no further
    /// change across the full range of inputs it touches.
    #[test]
    fn fix_is_idempotent() {
        let inputs = [
            // clean LF
            "---\nname: example\ndescription: A description.\n---\nBody.\n",
            // missing name
            "---\ndescription: A description.\n---\nBody.\n",
            // trailing whitespace
            "---\nname: example\ndescription: A description.\n---\nBody.   \n",
            // no trailing newline
            "---\nname: example\ndescription: A description.\n---\nBody.",
            // clean CRLF
            "---\r\nname: example\r\ndescription: A description.\r\n---\r\nBody.\r\n",
            // CRLF needing a fix
            "---\r\nname: example\r\ndescription: A description.\r\n---\r\nBody.   \r\n\r\n",
        ];
        for input in inputs {
            let once = fix_skill(&skill_path(), input);
            // Whatever the first pass produced, treat that as the current content.
            let after_first = once.contents.clone().unwrap_or_else(|| input.to_owned());
            let twice = fix_skill(&skill_path(), &after_first);
            assert!(
                twice.contents.is_none(),
                "second fix must be a no-op for input {input:?}; got {:?}",
                twice.contents
            );
            assert!(
                twice.changes.is_empty(),
                "second fix must report no changes for input {input:?}; got {:?}",
                twice.changes
            );
        }
    }
}
