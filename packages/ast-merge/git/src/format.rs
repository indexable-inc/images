use snafu::ResultExt as _;

use crate::types::DisplaySettings;

pub struct ConflictInput<'a> {
    pub left: &'a str,
    pub base: Option<&'a str>,
    pub right: &'a str,
}

#[must_use]
pub fn conflict(input: &ConflictInput<'_>, settings: &DisplaySettings) -> String {
    let left = input.left;
    let base = input.base;
    let right = input.right;
    let mut output = String::new();

    output.push_str(&"<".repeat(settings.marker_size));
    output.push(' ');
    output.push_str(&settings.left_name);
    output.push('\n');
    output.push_str(left);
    if !left.ends_with('\n') && !left.is_empty() {
        output.push('\n');
    }

    if settings.diff3_style
        && let Some(base_content) = base
    {
        output.push_str(&"|".repeat(settings.marker_size));
        output.push(' ');
        output.push_str(&settings.base_name);
        output.push('\n');
        output.push_str(base_content);
        if !base_content.ends_with('\n') && !base_content.is_empty() {
            output.push('\n');
        }
    }

    output.push_str(&"=".repeat(settings.marker_size));
    output.push('\n');

    output.push_str(right);
    if !right.ends_with('\n') && !right.is_empty() {
        output.push('\n');
    }

    output.push_str(&">".repeat(settings.marker_size));
    output.push(' ');
    output.push_str(&settings.right_name);
    output.push('\n');

    output
}

#[derive(Debug, snafu::Snafu)]
#[snafu(visibility(pub))]
pub enum RevisionError {
    #[snafu(display("failed to read revision from {path}"))]
    Read {
        path: String,
        source: std::io::Error,
    },

    #[snafu(display("failed to write merge result to {path}"))]
    Write {
        path: String,
        source: std::io::Error,
    },
}

/// Read a revision's contents from `path`.
///
/// # Errors
/// Returns an error if the file cannot be read.
pub fn read_revision(path: &std::path::Path) -> Result<String, RevisionError> {
    std::fs::read_to_string(path).context(ReadSnafu {
        path: path.display().to_string(),
    })
}

/// Write merged `content` to `path`.
///
/// # Errors
/// Returns an error if the file cannot be written.
pub fn write_result(path: &std::path::Path, content: &str) -> Result<(), RevisionError> {
    std::fs::write(path, content).context(WriteSnafu {
        path: path.display().to_string(),
    })
}

#[must_use]
pub fn extract_oid_from_marker(marker: &str) -> Option<&str> {
    marker
        .strip_prefix('<')
        .map(|s| s.trim_start_matches('<'))
        .and_then(|s| s.trim().split(':').next())
        .map(str::trim)
}
