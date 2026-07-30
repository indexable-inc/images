//! Test-only corpus builder. Every test that needs a corpus needs files on
//! disk, because discovery is a directory listing; this makes one in a temp
//! directory rather than each test hand-rolling it.

use crate::{
    discover::{self, Corpus, Root},
    model,
};
use chrono::{DateTime, Utc};
use std::path::PathBuf;

/// Fixed clock every test ranks and lints against, so no assertion depends on
/// the day it runs.
#[must_use]
pub fn fixed_now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-07-29T00:00:00Z")
        .expect("a literal RFC 3339 timestamp")
        .with_timezone(&Utc)
}

/// A `validated:` block dated at [`fixed_now`]. Written by [`Repo::memory`]
/// wherever the frontmatter contains the token `validated_today`, which keeps
/// the common "this memory is current" fixture to one word.
#[must_use]
pub fn validated_today() -> String {
    format!(
        "validated:\n  - at: {at}\n    by: test\n    how: the fixture\n    ok: true\n",
        at = model::format_timestamp(fixed_now()),
    )
}

/// A temp repo with a `.memories` directory.
pub struct Repo {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Repo {
    #[must_use]
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join(discover::MEMORIES_DIR_NAME))
            .expect("a `.memories` directory");
        Self { _dir: dir, root }
    }

    #[must_use]
    pub fn memories_dir(&self) -> PathBuf {
        self.root.join(discover::MEMORIES_DIR_NAME)
    }

    #[must_use]
    pub fn memory_path(&self, slug: &str) -> PathBuf {
        self.memories_dir().join(format!("{slug}.md"))
    }

    /// Write a file into `.memories` verbatim, for the malformed cases.
    pub fn raw(&self, file_name: &str, contents: &str) {
        std::fs::write(self.memories_dir().join(file_name), contents).expect("writing a memory");
    }

    /// Write a memory with a `tldr` derived from its slug, so no fixture trips
    /// the duplicate-`tldr` rule by accident.
    pub fn memory(&self, slug: &str, frontmatter: &str, body: &str) {
        let frontmatter = frontmatter.replace("validated_today", &validated_today());
        self.raw(
            &format!("{slug}.md"),
            &format!("---\ntldr: A memory about {slug}\n{frontmatter}---\n{body}"),
        );
    }

    /// Write a memory in a grouping subdirectory, one level down.
    pub fn group_memory(&self, group: &str, slug: &str, frontmatter: &str, body: &str) {
        self.write_at(&format!("{group}/{slug}.md"), slug, frontmatter, body);
    }

    /// Write a memory deeper than the format allows, for the rule that catches
    /// it.
    pub fn buried_memory(&self, groups: &str, slug: &str, frontmatter: &str, body: &str) {
        self.write_at(&format!("{groups}/{slug}.md"), slug, frontmatter, body);
    }

    fn write_at(&self, relative: &str, slug: &str, frontmatter: &str, body: &str) {
        let path = self.memories_dir().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("a group directory");
        }
        let frontmatter = frontmatter.replace("validated_today", &validated_today());
        std::fs::write(
            path,
            format!("---\ntldr: A memory about {slug}\n{frontmatter}---\n{body}"),
        )
        .expect("writing a memory");
    }

    /// Write a file elsewhere in the repo, for `based_on` targets.
    pub fn file(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("a parent directory");
        }
        std::fs::write(path, contents).expect("writing a repo file");
    }

    /// Write the closed topic set.
    pub fn topics(&self, topics: &[&str]) {
        std::fs::write(
            self.memories_dir().join(discover::TOPICS_FILE_NAME),
            topics.join("\n") + "\n",
        )
        .expect("writing topics.txt");
    }

    #[must_use]
    pub fn roots(&self) -> Vec<Root> {
        vec![Root::explicit(&self.root)]
    }

    #[must_use]
    pub fn load(&self) -> Corpus {
        discover::load(self.roots()).expect("loading a fixture corpus")
    }
}
