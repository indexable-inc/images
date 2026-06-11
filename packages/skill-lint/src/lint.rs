//! Pure SKILL.md analysis: no IO, no panics on bad input.
//!
//! The skillsaw Python tool we are replacing panicked whenever a line-attached
//! violation occurred, and reported malformed YAML opaquely. This core parses
//! the frontmatter with a real YAML parser (`serde_norway`) and turns every
//! failure into a [`Diagnostic`] rather than an abort.

use std::path::Path;

use serde::Serialize;

/// Description longer than this many characters is flagged. Skills are injected
/// into the model context verbatim, so an overlong description is wasted budget.
pub const DESCRIPTION_MAX_CHARS: usize = 1024;

/// Whole-file estimated-token ceiling. We approximate tokens as `len / 4`, the
/// rough bytes-per-token ratio for English/Markdown, to avoid pulling in a
/// tokenizer dependency for a soft budget warning.
pub const FILE_TOKEN_BUDGET: usize = 3000;

/// Bytes-per-token divisor for the rough estimate above.
const BYTES_PER_TOKEN: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    // `Info` is intentionally omitted: no rule emits it today, and the
    // workspace denies dead-code, so an unused variant would fail the build.
    // Add it back alongside the first informational rule that needs it.
}

impl Severity {
    const fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub path: String,
    /// 1-based line number when the diagnostic points at a specific line.
    pub line: Option<usize>,
    pub rule_id: String,
    pub message: String,
}

impl Diagnostic {
    fn new(
        severity: Severity,
        path: &str,
        line: Option<usize>,
        rule_id: &str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            path: path.to_owned(),
            line,
            rule_id: rule_id.to_owned(),
            message: message.into(),
        }
    }

    /// Human-readable single line: `severity path:line rule_id: message`.
    pub fn render(&self) -> String {
        let location = self
            .line
            .map_or_else(|| self.path.clone(), |line| format!("{}:{line}", self.path));
        format!(
            "{} {location} {}: {}",
            self.severity.label(),
            self.rule_id,
            self.message
        )
    }
}

/// Result of splitting leading frontmatter from the body. Shared with the fix
/// path so both agree on exactly what counts as frontmatter.
pub struct Frontmatter<'a> {
    /// YAML text between the delimiters.
    pub yaml: &'a str,
    /// 1-based line where the YAML block starts (the line after the opening
    /// `---`). Used to offset parser line numbers back to file lines.
    pub yaml_start_line: usize,
}

/// Split a leading `---` … `---` frontmatter block. Returns `None` when the
/// file does not begin with a frontmatter delimiter or has no closing one.
///
/// The YAML is sliced from the original string by tracking each line's byte
/// span via `split_inclusive`, so the slice stays exact (no reconstruction of
/// the terminator width that could drift on `\r\n`).
pub fn split_frontmatter(contents: &str) -> Option<Frontmatter<'_>> {
    let mut lines = contents.lines();
    // A leading delimiter must be the very first line (bare `---`).
    if lines.next()?.trim_end() != "---" {
        return None;
    }

    // Byte offset just past the opening `---` line and its newline.
    let yaml_begin = contents.find('\n')? + 1;
    let rest = &contents[yaml_begin..];

    // `split_inclusive` keeps each line's terminator, so the running offset is
    // always an exact byte position into `rest`.
    let mut offset = 0usize;
    for chunk in rest.split_inclusive('\n') {
        if chunk.trim_end() == "---" {
            return Some(Frontmatter {
                yaml: &rest[..offset],
                // File line 1 = opening `---`; YAML block begins on file line 2.
                yaml_start_line: 2,
            });
        }
        offset += chunk.len();
    }
    None
}

/// Lint a single SKILL.md given its path and contents. Pure: never reads the
/// filesystem and never panics on malformed input.
pub fn lint_skill(path: &Path, contents: &str) -> Vec<Diagnostic> {
    let path_str = path.display().to_string();
    let mut diagnostics = Vec::new();

    let Some(frontmatter) = split_frontmatter(contents) else {
        diagnostics.push(Diagnostic::new(
            Severity::Error,
            &path_str,
            Some(1),
            "skill-frontmatter",
            "missing frontmatter (expected a leading `---` … `---` block)",
        ));
        return diagnostics;
    };

    // THE feature: a real YAML parse, with the parser's own message and a file
    // line number, instead of skillsaw's opaque "malformed YAML" string. The
    // motivating trigger is a `description:` value containing a bare `: `,
    // which YAML reads as a nested mapping and breaks the document.
    let value: serde_norway::Value = match serde_norway::from_str(frontmatter.yaml) {
        Ok(value) => value,
        Err(error) => {
            // serde_norway line numbers are 1-based within the parsed block;
            // offset back to the file by the block's start line.
            let line = error
                .location()
                .map(|location| location.line() + frontmatter.yaml_start_line - 1);
            diagnostics.push(Diagnostic::new(
                Severity::Error,
                &path_str,
                line,
                "skill-frontmatter",
                format!("invalid YAML frontmatter: {error}"),
            ));
            return diagnostics;
        }
    };

    let serde_norway::Value::Mapping(mapping) = &value else {
        diagnostics.push(Diagnostic::new(
            Severity::Error,
            &path_str,
            Some(frontmatter.yaml_start_line),
            "skill-frontmatter",
            "frontmatter must be a YAML mapping (key: value pairs)",
        ));
        return diagnostics;
    };

    let name = string_field(mapping, "name");
    let description = string_field(mapping, "description");

    if name.is_none_or(str::is_empty) {
        diagnostics.push(Diagnostic::new(
            Severity::Error,
            &path_str,
            None,
            "skill-name",
            "frontmatter is missing a non-empty `name` string",
        ));
    }

    if description.is_none_or(str::is_empty) {
        diagnostics.push(Diagnostic::new(
            Severity::Error,
            &path_str,
            None,
            "skill-description",
            "frontmatter is missing a non-empty `description` string",
        ));
    }

    if let (Some(name), Some(dir)) = (name, parent_dir_name(path))
        && !name.is_empty()
        && name != dir
    {
        diagnostics.push(Diagnostic::new(
            Severity::Warning,
            &path_str,
            None,
            "skill-name-matches-dir",
            format!("`name` is \"{name}\" but the skill directory is \"{dir}\""),
        ));
    }

    if let Some(description) = description {
        let chars = description.chars().count();
        if chars > DESCRIPTION_MAX_CHARS {
            diagnostics.push(Diagnostic::new(
                Severity::Warning,
                &path_str,
                None,
                "skill-description-length",
                format!(
                    "description is {chars} chars, over the {DESCRIPTION_MAX_CHARS}-char threshold"
                ),
            ));
        }
    }

    let estimated_tokens = contents.len() / BYTES_PER_TOKEN;
    if estimated_tokens > FILE_TOKEN_BUDGET {
        diagnostics.push(Diagnostic::new(
            Severity::Warning,
            &path_str,
            None,
            "skill-file-budget",
            format!(
                "file is ~{estimated_tokens} estimated tokens, over the {FILE_TOKEN_BUDGET}-token budget"
            ),
        ));
    }

    diagnostics
}

/// Read a mapping field as a borrowed string, ignoring non-string values.
fn string_field<'a>(mapping: &'a serde_norway::Mapping, key: &str) -> Option<&'a str> {
    mapping
        .get(serde_norway::Value::String(key.to_owned()))
        .and_then(serde_norway::Value::as_str)
}

/// Name of the directory that directly contains the SKILL.md.
fn parent_dir_name(path: &Path) -> Option<&str> {
    path.parent()?.file_name()?.to_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_path() -> std::path::PathBuf {
        std::path::PathBuf::from("/repo/skills/example/SKILL.md")
    }

    #[test]
    fn valid_skill_has_no_diagnostics() {
        let contents = "---\nname: example\ndescription: A short, valid description.\n---\nBody.\n";
        let diagnostics = lint_skill(&skill_path(), contents);
        assert!(
            diagnostics.is_empty(),
            "expected clean, got {diagnostics:?}"
        );
    }

    #[test]
    fn malformed_yaml_bare_colon_space_is_one_error_with_line() {
        // The exact skillsaw trigger: a bare `: ` inside the description value
        // makes YAML read a nested mapping and the document fails to parse.
        let contents = concat!(
            "---\n",
            "name: example\n",
            "description: it does not block: it launches more work\n",
            "---\n",
            "Body.\n",
        );
        let diagnostics = lint_skill(&skill_path(), contents);
        assert_eq!(diagnostics.len(), 1, "got {diagnostics:?}");
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.rule_id, "skill-frontmatter");
        assert!(diagnostic.line.is_some(), "expected a line number");
        assert!(
            diagnostic.message.contains("invalid YAML"),
            "message: {}",
            diagnostic.message
        );
    }

    #[test]
    fn missing_name_yields_skill_name_error() {
        let contents = "---\ndescription: A description.\n---\nBody.\n";
        let diagnostics = lint_skill(&skill_path(), contents);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.rule_id == "skill-name" && d.severity == Severity::Error),
            "got {diagnostics:?}"
        );
    }

    #[test]
    fn name_not_matching_dir_is_warning_only() {
        let contents = "---\nname: other\ndescription: A description.\n---\nBody.\n";
        let diagnostics = lint_skill(&skill_path(), contents);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.rule_id == "skill-name-matches-dir" && d.severity == Severity::Warning),
            "got {diagnostics:?}"
        );
        assert!(
            !diagnostics.iter().any(|d| d.severity == Severity::Error),
            "no errors expected, got {diagnostics:?}"
        );
    }

    #[test]
    fn overlong_description_is_warning_only() {
        let long = "x".repeat(DESCRIPTION_MAX_CHARS + 1);
        let contents = format!("---\nname: example\ndescription: {long}\n---\nBody.\n");
        let diagnostics = lint_skill(&skill_path(), &contents);
        assert!(
            diagnostics
                .iter()
                .any(|d| d.rule_id == "skill-description-length"
                    && d.severity == Severity::Warning),
            "got {diagnostics:?}"
        );
        assert!(
            !diagnostics.iter().any(|d| d.severity == Severity::Error),
            "no errors expected, got {diagnostics:?}"
        );
    }

    #[test]
    fn missing_frontmatter_is_error() {
        let contents = "Just a body, no frontmatter.\n";
        let diagnostics = lint_skill(&skill_path(), contents);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].rule_id, "skill-frontmatter");
        assert_eq!(diagnostics[0].severity, Severity::Error);
    }
}
