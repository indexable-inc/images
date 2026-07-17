//! Target-repo PR template discovery and rendering.
//!
//! When the upstream repo ships a PR template, `upstream-pr` renders the PR
//! body INTO the template's `## ` sections instead of pasting a free-form
//! body over it: each section is filled from the source that owns it
//! (Description <- the commit body, release notes <- the patch's
//! `releaseNotes` from the fork mapping, additional notes <- `prExtra` + the
//! attribution block). A section this mapping does not recognize, or a
//! release-notes section for a patch that declares no `releaseNotes`,
//! REFUSES loudly: a template-noncompliant PR reads as low-effort to the
//! maintainers receiving it (nushell/nushell#18549), so there is no silent
//! fallback.

use std::path::{Path, PathBuf};

use color_eyre::eyre::{Result, bail};

/// The template locations GitHub resolves, in resolution order. The scratch
/// repo has the upstream default-branch tip checked out, so its working tree
/// is the source of truth.
const LOCATIONS: [&str; 6] = [
    ".github/pull_request_template.md",
    ".github/PULL_REQUEST_TEMPLATE.md",
    "pull_request_template.md",
    "PULL_REQUEST_TEMPLATE.md",
    "docs/pull_request_template.md",
    "docs/PULL_REQUEST_TEMPLATE.md",
];

/// The target repo's PR template file in the scratch checkout, or `None`
/// when the repo ships no template (the plain body composition then applies).
#[must_use]
pub fn find(scratch: &Path) -> Option<PathBuf> {
    LOCATIONS
        .into_iter()
        .map(|candidate| scratch.join(candidate))
        .find(|p| p.exists())
}

/// The content sources that fill a template's sections.
///
/// `release_notes` is optional at the type level because the fork mapping's
/// `releaseNotes` is optional; whether its absence is an error depends on
/// the template (only a template demanding a release-notes section refuses).
pub struct Sections {
    /// The patch's commit-message body (fills a Description section).
    pub description: String,
    /// `patches.<patch>.releaseNotes` from the fork mapping, when declared.
    pub release_notes: Option<String>,
    /// `prExtra` + the AI-attribution block: the "anything else reviewers
    /// should know" content (fills an additional-notes section).
    pub notes: String,
}

/// Render the PR body into the template's `## ` sections.
///
/// # Errors
/// Fails when the template has no `## ` sections, demands a release-notes
/// section for a patch with no `releaseNotes`, or carries a section this
/// mapping does not recognize; the caller must then NOT open the PR.
pub fn render(template: &str, pkg: &str, target: &str, sections: &Sections) -> Result<String> {
    let headings: Vec<&str> = template
        .lines()
        .filter_map(|l| l.strip_prefix("## "))
        .map(str::trim)
        .collect();
    if headings.is_empty() {
        bail!(
            "upstream-pr: {pkg}: the target repo has a PR template with no `## ` sections; this tool cannot render into it. Open the PR by hand, following the template."
        );
    }
    let mut rendered: Vec<String> = Vec::with_capacity(headings.len());
    for heading in headings {
        let lower = heading.to_lowercase();
        let content = if lower.contains("description") {
            sections.description.clone()
        } else if lower.contains("user-facing") || lower.contains("release note") {
            let Some(notes) = &sections.release_notes else {
                bail!(
                    "upstream-pr: {pkg}: the target repo's PR template requires a '{heading}' section, but {target} declares no `releaseNotes` in the fork mapping (lib/fork-packages.nix). Write the user-facing change in release-note style (or 'n/a') there; NOT opening a template-noncompliant PR."
                );
            };
            notes.trim().to_owned()
        } else if lower.contains("additional note") || lower == "notes" {
            sections.notes.clone()
        } else {
            bail!(
                "upstream-pr: {pkg}: the target repo's PR template has a '{heading}' section this tool does not know how to fill. Extend the section mapping in packages/upstream-sync/src/template.rs or open the PR by hand; NOT opening a template-noncompliant PR."
            );
        };
        rendered.push(format!("## {heading}\n\n{content}"));
    }
    Ok(rendered.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn sections() -> Sections {
        Sections {
            description: "Why the change.".to_owned(),
            release_notes: Some("`ls -l` grows a column.\n".to_owned()),
            notes: "Related: #1.\n\n---\n\nAttribution.".to_owned(),
        }
    }

    #[test]
    fn renders_known_sections_in_template_order() {
        let template = "<!-- intro comment -->\n## Description\n(explain)\n\n## User-facing changes (Release notes)\n\n## Additional notes\n";
        let body = render(template, "fake", "0001-x.patch", &sections()).unwrap();
        assert_eq!(
            body,
            "## Description\n\nWhy the change.\n\n## User-facing changes (Release notes)\n\n`ls -l` grows a column.\n\n## Additional notes\n\nRelated: #1.\n\n---\n\nAttribution."
        );
    }

    #[test]
    fn missing_release_notes_refuses_loudly() {
        let template = "## Description\n\n## User-facing changes (Release notes)\n";
        let mut missing = sections();
        missing.release_notes = None;
        let err = render(template, "fake", "0001-x.patch", &missing).unwrap_err();
        assert!(
            err.to_string().contains("declares no `releaseNotes`"),
            "{err}"
        );
    }

    #[test]
    fn unknown_section_refuses_loudly() {
        let template = "## Description\n\n## Benchmarks\n";
        let err = render(template, "fake", "0001-x.patch", &sections()).unwrap_err();
        assert!(
            err.to_string().contains("does not know how to fill"),
            "{err}"
        );
    }

    #[test]
    fn headingless_template_refuses_loudly() {
        let err = render("just prose\n", "fake", "0001-x.patch", &sections()).unwrap_err();
        assert!(err.to_string().contains("no `## ` sections"), "{err}");
    }

    #[test]
    fn find_prefers_the_dot_github_location() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(find(tmp.path()), None);
        fs::write(tmp.path().join("PULL_REQUEST_TEMPLATE.md"), "## D\n").unwrap();
        fs::create_dir(tmp.path().join(".github")).unwrap();
        fs::write(
            tmp.path().join(".github/pull_request_template.md"),
            "## D\n",
        )
        .unwrap();
        assert_eq!(
            find(tmp.path()),
            Some(tmp.path().join(".github/pull_request_template.md"))
        );
    }
}
