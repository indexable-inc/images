//! Adapter turning a git repository's commit history into embeddable, tagged
//! [`source_meta`] documents for the multi-source `search` store.
//!
//! # Grain
//! One [`Document`] per **commit**, embedding the commit message (subject +
//! body). The diff is intentionally not embedded: across full history it is
//! huge and costly, and it already lives in git, reachable via the `commit` tag.
//! `external_id = "git:{repo}:{sha}"`, so re-ingesting a repo only uploads its
//! new commits via the content-hash reconcile.
//!
//! # Tags
//! Every document carries the common header (`source`, `external_id`,
//! `content_hash`, `title`, `timestamp`) plus `repo`, `commit`, `author_name`,
//! and `author_email`, so a query can scope to a repo, author, or time.

#![forbid(unsafe_code)]

mod error;
mod record;

use std::path::Path;
use std::process::Command;

use source_meta::{Document, Source, SourceAdapter};

pub use crate::error::Error;
use crate::error::{GitFailedSnafu, ParseSnafu, Result, SpawnSnafu};
pub use crate::record::Commit;
use snafu::ResultExt as _;

/// The `source` tag every commit document carries.
pub const SOURCE_TAG: &str = "git";

/// `git log` pretty format: fields separated by US (`0x1f`), commits by NUL
/// (via `-z`). Order: full SHA, author name, author email, author epoch
/// seconds, subject, body.
const FORMAT: &str = "%H%x1f%an%x1f%ae%x1f%at%x1f%s%x1f%b";

/// A repository's parsed commit history ready to project into documents.
///
/// Construct with [`GitLog::open`], which shells out to `git log` once. Parsing
/// happens up front so [`SourceAdapter::documents`] is cheap to start.
#[derive(Debug)]
#[must_use]
pub struct GitLog {
    commits: Vec<Commit>,
}

impl GitLog {
    /// Read every commit reachable from the repository's current `HEAD`.
    ///
    /// # Errors
    /// Returns an error if `git` cannot be spawned, `git log` exits non-zero, or
    /// its output cannot be parsed.
    pub fn open(repo: &Path) -> Result<Self> {
        let repo_slug = slug(repo);
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["log", "--no-color", "-z", &format!("--format={FORMAT}")])
            .output()
            .context(SpawnSnafu {
                repo: repo.to_path_buf(),
            })?;
        if !output.status.success() {
            return GitFailedSnafu {
                repo: repo.to_path_buf(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
            .fail();
        }
        let commits = parse_log(&output.stdout, &repo_slug)?;
        Ok(Self { commits })
    }

    /// Number of parsed commits.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.commits.len()
    }

    /// Whether no commits were parsed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.commits.is_empty()
    }
}

impl SourceAdapter for GitLog {
    type Error = Error;

    fn source(&self) -> Source {
        Source::new(SOURCE_TAG)
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        self.commits.clone().into_iter().map(Commit::into_document)
    }
}

/// Parse `git log -z --format=<FORMAT>` output into commits. Records are
/// NUL-separated; fields within a record are US-separated.
fn parse_log(bytes: &[u8], repo: &str) -> Result<Vec<Commit>> {
    let text = String::from_utf8_lossy(bytes);
    let mut commits = Vec::new();
    for raw in text.split('\0') {
        // The body of the previous commit can end in newlines before the NUL;
        // strip any leading newline so the SHA field starts clean.
        let record = raw.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(6, '\u{1f}');
        let sha = field(&mut fields, "sha")?;
        let author_name = field(&mut fields, "author_name")?;
        let author_email = field(&mut fields, "author_email")?;
        let at = field(&mut fields, "timestamp")?;
        let subject = field(&mut fields, "subject")?;
        let body = fields.next().unwrap_or("");
        let timestamp = at.trim().parse::<i64>().map_err(|_err| {
            ParseSnafu {
                detail: format!("bad timestamp {at:?}"),
            }
            .build()
        })?;
        commits.push(Commit {
            repo: repo.to_owned(),
            sha: sha.trim().to_owned(),
            author_name: author_name.to_owned(),
            author_email: author_email.to_owned(),
            timestamp,
            subject: subject.to_owned(),
            body: body.trim_end().to_owned(),
        });
    }
    Ok(commits)
}

/// Take the next US-separated field, or fail with a layout error.
fn field<'a>(fields: &mut impl Iterator<Item = &'a str>, name: &str) -> Result<&'a str> {
    fields.next().ok_or_else(|| {
        ParseSnafu {
            detail: format!("missing field {name}"),
        }
        .build()
    })
}

/// Derive a repo slug from the `origin` remote (`org/repo`), falling back to the
/// directory name when there is no remote.
fn slug(repo: &Path) -> String {
    let from_remote = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| slug_from_url(String::from_utf8_lossy(&output.stdout).trim()));
    from_remote.unwrap_or_else(|| {
        repo.file_name().map_or_else(
            || "local".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    })
}

/// Reduce a remote URL to `org/repo`, handling both `git@host:org/repo.git` and
/// `https://host/org/repo.git` forms.
fn slug_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches('/').trim_end_matches(".git");
    let parts: Vec<&str> = trimmed
        .split(['/', ':'])
        .filter(|part| !part.is_empty())
        .collect();
    let len = parts.len();
    (len >= 2).then(|| format!("{}/{}", parts[len - 2], parts[len - 1]))
}

#[cfg(test)]
mod tests {
    use super::{parse_log, slug_from_url};

    #[test]
    fn parses_nul_separated_records() {
        let bytes =
            b"abc123def\x1fAlice\x1falice@x.com\x1f1700000000\x1fFix bug\x1fLonger\nbody\x00\
                      fff999000\x1fBob\x1fbob@y.com\x1f1700000100\x1fAdd feature\x1f\x00";
        let commits = parse_log(bytes, "org/repo").expect("parse");
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].sha, "abc123def");
        assert_eq!(commits[0].author_name, "Alice");
        assert_eq!(commits[0].timestamp, 1_700_000_000);
        assert_eq!(commits[0].subject, "Fix bug");
        assert_eq!(commits[0].body, "Longer\nbody");
        assert_eq!(commits[1].body, "");
    }

    #[test]
    fn projects_document_with_git_tags() {
        let bytes = b"abc123def456\x1fAlice\x1falice@x.com\x1f1700000000\x1fFix bug\x1f\x00";
        let doc = parse_log(bytes, "org/repo").expect("parse")[0]
            .clone()
            .into_document()
            .expect("doc");
        assert_eq!(doc.external_id, "git:org/repo:abc123def456");
        assert_eq!(doc.meta_json["source"], "git");
        assert_eq!(doc.meta_json["repo"], "org/repo");
        assert_eq!(doc.meta_json["commit"], "abc123def456");
        assert_eq!(doc.meta_json["author_email"], "alice@x.com");
        assert_eq!(doc.meta_json["timestamp"], 1_700_000_000_i64);
        // Empty body → the embedded message is just the subject.
        assert_eq!(doc.body, b"Fix bug");
    }

    #[test]
    fn slug_from_both_url_forms() {
        assert_eq!(
            slug_from_url("git@github.com:org/repo.git").as_deref(),
            Some("org/repo")
        );
        assert_eq!(
            slug_from_url("https://github.com/org/repo.git").as_deref(),
            Some("org/repo")
        );
        assert_eq!(
            slug_from_url("https://github.com/org/repo/").as_deref(),
            Some("org/repo")
        );
    }
}
