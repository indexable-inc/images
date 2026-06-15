//! Adapter turning Claude Code agent transcripts into embeddable, tagged
//! [`source_meta`] documents for the multi-source `search` store.
//!
//! # Grain
//! One [`Document`] per transcript **message** (a `user`/`assistant` line that
//! carries content). `external_id = "claude:{session_id}:{uuid}"`, so an
//! append-only transcript re-ingests only its new messages: the content-hash
//! reconcile in `search-core` skips everything already uploaded.
//!
//! A tool call and its output stay in one document: each `tool_result` is folded
//! into the `tool_use` message that produced it (matched by `tool_use_id`), so a
//! tool result is never indexed as a standalone, context-free chunk.
//!
//! # Tags
//! Every document's flat metadata carries the common header (`source`,
//! `external_id`, `content_hash`, `title`, `timestamp`) plus the agent-history
//! filter tags (`host`, `user`, `project`, `session_id`, `message_uuid`,
//! `parent_uuid`, `role`, `record_type`, `model`, `cwd`, `git_branch`,
//! `tool_name`, token counts), so a query can scope to a machine, user,
//! project, session, or role.

#![forbid(unsafe_code)]

mod error;
mod record;
mod transcript;

use std::path::{Path, PathBuf};

use snafu::ResultExt as _;
use source_meta::{Document, Source, SourceAdapter};

pub use crate::error::Error;
use crate::error::{HostNameSnafu, ReadDirSnafu, Result};
pub use crate::record::Message;
use crate::record::MessageOrigin;

/// The `source` tag every Claude transcript document carries.
pub const SOURCE_TAG: &str = "claude_history";

/// A set of parsed Claude transcript messages ready to project into documents.
///
/// Construct with [`ClaudeHistoryExport::open`], which recursively reads every
/// `*.jsonl` transcript under a directory (e.g. `~/.claude/projects`). Parsing
/// happens up front so [`SourceAdapter::documents`] is cheap to start.
#[derive(Debug)]
#[must_use]
pub struct ClaudeHistoryExport {
    messages: Vec<Message>,
}

impl ClaudeHistoryExport {
    /// Open and parse every transcript under `dir`, tagging each message with an
    /// explicit `host` and `user`. The fleet sync binary uses this so it can tag
    /// per-machine; [`open`](Self::open) resolves them automatically.
    ///
    /// # Errors
    /// Returns an error if a directory cannot be listed. A transcript that
    /// cannot be read is logged and skipped, not fatal: one unreadable file must
    /// not drop every other transcript for this account.
    pub fn open_with(dir: &Path, host: &str, user: &str) -> Result<Self> {
        let mut files = Vec::new();
        collect_transcripts(dir, &mut files)?;

        let mut messages = Vec::new();
        for file in files {
            let origin = origin_for(&file, host, user);
            match transcript::parse(&file, &origin) {
                Ok(parsed) => messages.extend(parsed),
                Err(error) => eprintln!("[claude] skipping transcript {}: {error}", file.display()),
            }
        }
        Ok(Self { messages })
    }

    /// Open every transcript under `dir`, resolving `host` (via `gethostname`)
    /// and `user` (the OS user) automatically. This is the entry point the
    /// `indexer`'s `--claude-dir` (and `--local`) uses.
    ///
    /// # Errors
    /// Returns an error if the host name cannot be resolved, or a transcript
    /// cannot be read or parsed.
    pub fn open(dir: &Path) -> Result<Self> {
        let host = hostname()?;
        let user = os_user();
        Self::open_with(dir, &host, &user)
    }

    /// Number of parsed messages.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether no messages were parsed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// The parsed messages, in transcript order across all sessions. The R2
    /// parquet sink consumes these as rows; the Mixedbread sink uses the
    /// [`SourceAdapter`] projection instead.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}

impl SourceAdapter for ClaudeHistoryExport {
    type Error = Error;

    fn source(&self) -> Source {
        Source::new(SOURCE_TAG)
    }

    fn documents(&self) -> impl Iterator<Item = Result<Document, Error>> + Send {
        // Clone into an owned iterator so the result is `'static + Send`,
        // independent of `&self` (mirrors the slack/linear adapters).
        self.messages
            .clone()
            .into_iter()
            .map(Message::into_document)
    }
}

/// Recursively collect `*.jsonl` transcript files under `dir`.
///
/// The top-level `dir` is followed even when it is a symlink: callers name it
/// explicitly, and `~/.claude/projects` is itself a symlink in some setups (it
/// points at the real store). Inside the tree, symlinks are never followed —
/// both symlinked directories and symlinked files are skipped — so a symlink
/// planted *within* a transcript tree cannot redirect the read. No-follow
/// traversal also means there are no symlink cycles to break.
///
/// This does NOT vet the root or its ancestor directories. When running
/// privileged over other accounts' homes (`CAP_DAC_READ_SEARCH` on the fleet),
/// the caller must ensure the root path has no symlinked component, or a user
/// could point the root itself at another account's files (the confused-deputy
/// class; see ix `history-ship`'s symlink finding). The indexer does this with
/// its `safe_path_under` resolver before calling in.
///
/// A missing directory yields nothing; a permission or I/O fault is a real error
/// (not a silently empty success). Absence is normal: most homes have no Claude
/// history, and the privileged fleet run walks many of them.
fn collect_transcripts(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    // `read_dir` follows a symlinked `dir` (the explicitly named root); the
    // per-entry `file_type` below reports the entry itself without following, so
    // nothing reached through the tree can be a symlink.
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).context(ReadDirSnafu {
                path: dir.to_path_buf(),
            });
        }
    };
    for entry in entries {
        let entry = entry.context(ReadDirSnafu {
            path: dir.to_path_buf(),
        })?;
        let file_type = entry.file_type().context(ReadDirSnafu {
            path: dir.to_path_buf(),
        })?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_transcripts(&path, out)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
    Ok(())
}

/// Derive a file's fallback identity tags: project from the parent directory
/// name, session from the file stem. A line's own `cwd`/`sessionId` override
/// these when present.
fn origin_for(file: &Path, host: &str, user: &str) -> MessageOrigin {
    let project = file
        .parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let session_id = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    MessageOrigin {
        host: host.to_owned(),
        user: user.to_owned(),
        project,
        session_id,
    }
}

/// Resolve the host name for record tagging.
fn hostname() -> Result<String> {
    let raw = nix::unistd::gethostname()
        .map_err(std::io::Error::from)
        .context(HostNameSnafu)?;
    Ok(raw.to_string_lossy().into_owned())
}

/// The OS user owning the process, for the `user` tag.
fn os_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "tests assert observable filesystem outcomes"
    )]

    use std::path::{Path, PathBuf};

    use super::collect_transcripts;

    fn collect(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        collect_transcripts(dir, &mut out).expect("collect");
        out.sort();
        out
    }

    #[test]
    fn missing_dir_yields_nothing() {
        assert!(collect(Path::new("/nonexistent-claude-root-xyz")).is_empty());
    }

    #[test]
    fn finds_nested_transcripts() {
        let temp = tempfile::tempdir().expect("tempdir");
        let nested = temp.path().join("proj").join("sess");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(nested.join("a.jsonl"), b"{}").expect("write a");
        std::fs::write(temp.path().join("b.jsonl"), b"{}").expect("write b");
        std::fs::write(temp.path().join("notes.txt"), b"x").expect("write notes");

        let found = collect(temp.path());
        assert_eq!(found.len(), 2, "only .jsonl files, collected recursively");
        assert!(
            found
                .iter()
                .all(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        );
    }

    #[test]
    fn symlinked_leaf_transcript_is_skipped() {
        let temp = tempfile::tempdir().expect("tempdir");
        // Stands in for a sensitive target the privileged walk must not follow.
        let secret = temp.path().join("secret-target");
        std::fs::write(&secret, b"{}").expect("write secret");
        std::os::unix::fs::symlink(&secret, temp.path().join("leak.jsonl")).expect("symlink");
        std::fs::write(temp.path().join("real.jsonl"), b"{}").expect("write real");

        let found = collect(temp.path());
        assert_eq!(
            found.len(),
            1,
            "the symlinked transcript must not be collected"
        );
        assert_eq!(
            found[0].file_name().and_then(|name| name.to_str()),
            Some("real.jsonl")
        );
    }

    #[test]
    fn symlinked_subdir_is_not_descended() {
        let temp = tempfile::tempdir().expect("tempdir");
        // A tree outside the root that a user-planted directory symlink could
        // otherwise redirect the privileged walk into.
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        std::fs::write(outside.join("secret.jsonl"), b"{}").expect("write secret");

        let root = temp.path().join("root");
        std::fs::create_dir_all(&root).expect("mkdir root");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");
        std::fs::write(root.join("real.jsonl"), b"{}").expect("write real");

        let found = collect(&root);
        assert_eq!(
            found.len(),
            1,
            "files under a symlinked subdir must not be collected"
        );
        assert_eq!(
            found[0].file_name().and_then(|name| name.to_str()),
            Some("real.jsonl")
        );
    }

    /// End-to-end hygiene proof: a transcript whose tool output carries a fake
    /// credential (constructed at test time — never a real key), ANSI escapes,
    /// a base64-ish blob, and a giant CI log comes out of the adapter's
    /// [`Document`](source_meta::Document) body sanitized, and the
    /// `content_hash` is computed over the sanitized bytes (so a re-sync sees
    /// previously ingested raw bodies as changed and re-uploads them clean).
    #[test]
    fn fake_secret_in_transcript_is_redacted_end_to_end() {
        use source_meta::SourceAdapter as _;

        let fake_key = format!("lin_api_{}", "Ab0".repeat(13));
        let big_output = format!(
            "\u{1b}[1mLOG-HEAD\u{1b}[0m {}\ncurl -H \"Authorization: {fake_key}\"\n{}LOG-TAIL",
            "QUJD+/=a".repeat(40),
            "one line of CI output\n".repeat(600),
        );
        let call = serde_json::json!({
            "type": "assistant", "uuid": "a1", "sessionId": "s1",
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "Bash",
                 "input": {"command": "./ci.sh"}}
            ]}
        });
        let result = serde_json::json!({
            "type": "user", "uuid": "u1", "sessionId": "s1",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": big_output}
            ]}
        });

        let temp = tempfile::tempdir().expect("tempdir");
        let proj = temp.path().join("proj");
        std::fs::create_dir_all(&proj).expect("mkdir");
        std::fs::write(proj.join("s1.jsonl"), format!("{call}\n{result}\n")).expect("write");

        let export =
            super::ClaudeHistoryExport::open_with(temp.path(), "test-host", "test-user")
                .expect("open");
        let documents: Vec<_> = export
            .documents()
            .collect::<Result<_, _>>()
            .expect("documents");
        assert_eq!(documents.len(), 1, "call and result fold into one document");
        let document = &documents[0];
        let body = String::from_utf8(document.body.clone()).expect("utf8 body");

        assert!(
            !body.contains(&fake_key),
            "the raw key must never be embedded: {body}"
        );
        assert!(body.contains("[redacted:linear_api_key]"), "{body}");
        assert!(!body.contains('\u{1b}'), "ANSI escapes stripped: {body}");
        assert!(body.contains("[blob 320 chars]"), "{body}");
        assert!(
            body.contains("[truncated"),
            "the giant tool_result is capped: {} chars",
            body.chars().count()
        );
        assert!(body.contains("LOG-TAIL"), "the tail survives the cap");
        assert_eq!(
            document.content_hash,
            source_meta::hash_body(&document.body),
            "content_hash is computed AFTER sanitation, over the embedded bytes"
        );
    }

    #[test]
    fn top_level_symlinked_root_is_followed() {
        // The user's own ~/.claude/projects is a symlink to the real store, so
        // the explicitly named root must still be read.
        let temp = tempfile::tempdir().expect("tempdir");
        let real = temp.path().join("real-store");
        std::fs::create_dir_all(&real).expect("mkdir real");
        std::fs::write(real.join("s.jsonl"), b"{}").expect("write s");
        let link = temp.path().join("projects");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        assert_eq!(collect(&link).len(), 1, "a symlinked root is followed");
    }
}
