//! Per-source input-file cursor for the scan path (ENG-2698).
//!
//! The hourly fleet run used to re-parse every user home's full transcript set
//! even when nothing had changed. This cursor remembers, per `(user, source)`,
//! the signature (size + mtime) of every input file a source would read, taken
//! on its last successful run; when the current signatures match exactly, the
//! source is skipped without opening a single transcript.
//!
//! The skip is all-or-nothing per source — one changed, added, or removed file
//! reparses the whole source — because the durable sinks consume the source's
//! COMPLETE document set: the parquet log is a full-file overwrite and the lake
//! derives tombstones from absences, so a partially parsed set would erase the
//! documents of every file it left out. Per-file signatures are still what
//! make the gate cheap and exact: deciding "unchanged" costs one `lstat` per
//! file, no reads.
//!
//! Correctness under rewrite: any size change (a shrink above all) or mtime
//! change differs from the stored signature and forces a reparse. Callers take
//! the snapshot BEFORE parsing and store it only after every sink succeeded,
//! so a file appended mid-run is re-read next run and a failed sink is never
//! buried under a fresh cursor. The cursor is advisory state: deleting the
//! directory (or a malformed file in it) just forces one full reingest.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// One input file's change signature: size plus mtime at the precision the
/// filesystem reports (nanoseconds where supported).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSig {
    /// File length in bytes; a shrink or growth always differs.
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch (negative for
    /// pre-epoch timestamps).
    pub mtime_ns: i128,
}

/// The signatures of every regular file a source would read, keyed by path.
pub type Snapshot = BTreeMap<String, FileSig>;

/// Stat every regular file under `inputs`: a file input is taken as-is, a
/// directory input is walked recursively. The named roots are followed even
/// when symlinked (callers name them explicitly, and e.g. `~/.claude/projects`
/// is itself a symlink in some setups) but nothing inside a tree is — the same
/// no-follow discipline as the adapters' own walks. Collecting more files than
/// an adapter parses is fine (a change in an unparsed file only forces a
/// spurious reparse); collecting fewer would let a change slip past a skip, so
/// the walk takes every regular file. A missing input contributes nothing.
///
/// # Errors
/// Returns an error if a directory cannot be listed or a file cannot be
/// stat'ed (other than not existing). Callers treat that as "no gate this
/// run": parse normally and let the adapter surface the real fault.
pub fn snapshot(inputs: &[&Path]) -> anyhow::Result<Snapshot> {
    let mut files = Snapshot::new();
    for input in inputs {
        // The root is followed (std::fs::metadata), entries below are not.
        let meta = match std::fs::metadata(input) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(anyhow::Error::new(error)
                    .context(format!("stat'ing the source input {}", input.display())));
            }
        };
        if meta.is_dir() {
            walk(input, &mut files)?;
        } else if meta.is_file() {
            files.insert(input.to_string_lossy().into_owned(), signature(&meta)?);
        }
    }
    Ok(files)
}

/// Recursively collect regular-file signatures under `dir`, skipping symlinks
/// (both files and directories) like the transcript adapters do.
fn walk(dir: &Path, out: &mut Snapshot) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("listing the source input directory {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("listing {}", dir.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("stat'ing {}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            walk(&path, out)?;
        } else if file_type.is_file() {
            let meta = entry
                .metadata()
                .with_context(|| format!("stat'ing {}", path.display()))?;
            out.insert(path.to_string_lossy().into_owned(), signature(&meta)?);
        }
    }
    Ok(())
}

/// Project a file's metadata to its change signature.
fn signature(meta: &std::fs::Metadata) -> anyhow::Result<FileSig> {
    let modified = meta.modified().context("reading a file's mtime")?;
    // Infallible widening: u64 seconds times 1e9 plus sub-second nanos fits
    // comfortably in i128.
    let nanos = |duration: std::time::Duration| {
        i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos())
    };
    let mtime_ns = match modified.duration_since(UNIX_EPOCH) {
        Ok(after) => nanos(after),
        // Pre-epoch mtimes are representable, just negative.
        Err(earlier) => -nanos(earlier.duration()),
    };
    Ok(FileSig {
        size: meta.len(),
        mtime_ns,
    })
}

/// The on-disk cursor store: one JSON file per `(user, source)` under `root`
/// (the indexer's state directory, `/var/lib/ix-indexer` on the fleet).
pub struct ScanCursor {
    root: PathBuf,
}

/// The serialized cursor file: the snapshot under a named key, so the format
/// can grow fields without breaking old files.
#[derive(Serialize, Deserialize)]
struct CursorFile {
    files: Snapshot,
}

impl ScanCursor {
    /// A cursor store rooted at `root`.
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Where one `(user, source)` cursor lives: `scan/user=<name>/<source>.json`
    /// for the fleet's `--user` scope, `scan/local/<source>.json` for the
    /// single-user path. `--user` names are validated to a charset with no
    /// `/`/`=`, so a name can neither escape the directory nor collide with
    /// the `local` scope.
    fn cursor_path(&self, user: Option<&str>, source: &str) -> PathBuf {
        let scope = user.map_or_else(|| "local".to_owned(), |name| format!("user={name}"));
        self.root
            .join("scan")
            .join(scope)
            .join(format!("{source}.json"))
    }

    /// Whether `current` matches the snapshot stored by the last successful
    /// run. An absent or malformed cursor file is "changed" (parse and
    /// overwrite), never an error: the cursor only ever saves work.
    #[must_use]
    pub fn unchanged(&self, user: Option<&str>, source: &str, current: &Snapshot) -> bool {
        let path = self.cursor_path(user, source);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "[cursor] failed to read {}; reparsing: {error}",
                        path.display()
                    );
                }
                return false;
            }
        };
        match serde_json::from_slice::<CursorFile>(&bytes) {
            Ok(stored) => stored.files == *current,
            Err(error) => {
                eprintln!(
                    "[cursor] malformed cursor file {}; reparsing: {error}",
                    path.display()
                );
                false
            }
        }
    }

    /// Persist `current` for `(user, source)` atomically (temp file + rename in
    /// the same directory), so a crash mid-write can never leave a truncated
    /// cursor. Call only after every sink succeeded, or a failed write would be
    /// skipped forever.
    pub fn store(&self, user: Option<&str>, source: &str, current: &Snapshot) -> anyhow::Result<()> {
        let path = self.cursor_path(user, source);
        let dir = path.parent().context("cursor path has a parent")?;
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating the cursor directory {}", dir.display()))?;
        let body = serde_json::to_vec(&CursorFile {
            files: current.clone(),
        })
        .context("serializing the cursor")?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, body)
            .with_context(|| format!("writing the cursor temp file {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("renaming the cursor into place at {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "tests assert observable filesystem outcomes"
    )]

    use std::fs::File;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    use super::{ScanCursor, Snapshot, snapshot};

    /// Shift a file's mtime forward by `secs` without touching its bytes, so a
    /// test can model "rewritten with identical length" deterministically
    /// (back-to-back writes can land in one filesystem timestamp granule).
    fn bump_mtime(path: &Path, secs: u64) {
        let file = File::options().write(true).open(path).expect("open");
        file.set_modified(SystemTime::now() + Duration::from_secs(secs))
            .expect("set mtime");
    }

    #[test]
    fn snapshot_walks_regular_files_and_skips_symlinks() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("projects");
        std::fs::create_dir_all(root.join("proj-a")).expect("mkdir");
        std::fs::write(root.join("proj-a").join("s1.jsonl"), b"{}\n").expect("write");
        std::fs::write(root.join("top.jsonl"), b"{}\n{}\n").expect("write");
        // A planted symlink inside the tree must not be followed (matching the
        // adapters' no-follow walks): its target's changes are invisible, just
        // as they are to the parse.
        let outside = temp.path().join("outside.jsonl");
        std::fs::write(&outside, b"{}\n").expect("write");
        std::os::unix::fs::symlink(&outside, root.join("planted.jsonl")).expect("symlink");

        let single = temp.path().join("history.jsonl");
        std::fs::write(&single, b"line\n").expect("write");
        let missing = temp.path().join("does-not-exist");

        let snap = snapshot(&[&root, &single, &missing]).expect("snapshot");
        let keys: Vec<&str> = snap.keys().map(String::as_str).collect();
        let mut expected = [
            root.join("proj-a").join("s1.jsonl"),
            root.join("top.jsonl"),
            single.clone(),
        ];
        expected.sort_unstable();
        let expected: Vec<&str> = expected.iter().map(|p| p.to_str().expect("utf8")).collect();
        assert_eq!(keys, expected);
        assert_eq!(snap[single.to_str().expect("utf8")].size, 5);
    }

    /// A one-transcript tree and its snapshot.
    struct Seeded {
        dir: std::path::PathBuf,
        snap: Snapshot,
    }

    /// Build a one-transcript tree and snapshot it.
    fn seeded(temp: &Path) -> Seeded {
        let dir = temp.join("projects");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("s.jsonl"), b"{\"a\":1}\n").expect("write");
        let snap = snapshot(&[&dir]).expect("snapshot");
        Seeded { dir, snap }
    }

    #[test]
    fn unchanged_only_after_store_and_any_file_change_invalidates() {
        let temp = tempfile::tempdir().expect("tempdir");
        let Seeded { dir, snap } = seeded(temp.path());
        let cursor = ScanCursor::new(temp.path().join("state"));

        // Bootstrap: nothing stored yet, so nothing is "unchanged".
        assert!(!cursor.unchanged(None, "claude", &snap));
        cursor.store(None, "claude", &snap).expect("store");
        assert!(cursor.unchanged(None, "claude", &snap));

        // Append: the size grows.
        let transcript = dir.join("s.jsonl");
        let mut bytes = std::fs::read(&transcript).expect("read");
        bytes.extend_from_slice(b"{\"a\":2}\n");
        std::fs::write(&transcript, &bytes).expect("append");
        let appended = snapshot(&[&dir]).expect("snapshot");
        assert!(
            !cursor.unchanged(None, "claude", &appended),
            "an appended transcript must reparse"
        );

        // Shrink: a rewrite smaller than before (the rewrite case the cursor
        // must catch even if mtime granularity hid the timestamp change).
        std::fs::write(&transcript, b"{}\n").expect("shrink");
        let shrunk = snapshot(&[&dir]).expect("snapshot");
        assert!(
            !cursor.unchanged(None, "claude", &shrunk),
            "a shrunk transcript must reparse"
        );

        // Same-size rewrite: only the mtime moves.
        std::fs::write(&transcript, b"{\"a\":1}\n").expect("rewrite");
        bump_mtime(&transcript, 5);
        let touched = snapshot(&[&dir]).expect("snapshot");
        assert!(
            !cursor.unchanged(None, "claude", &touched),
            "a same-size rewrite with a newer mtime must reparse"
        );

        // A new transcript appears.
        cursor.store(None, "claude", &touched).expect("store");
        assert!(cursor.unchanged(None, "claude", &touched));
        std::fs::write(dir.join("s2.jsonl"), b"{}\n").expect("write");
        let added = snapshot(&[&dir]).expect("snapshot");
        assert!(
            !cursor.unchanged(None, "claude", &added),
            "a new transcript must reparse"
        );

        // A transcript disappears (the lake needs the run to tombstone it).
        cursor.store(None, "claude", &added).expect("store");
        std::fs::remove_file(dir.join("s2.jsonl")).expect("remove");
        let removed = snapshot(&[&dir]).expect("snapshot");
        assert!(
            !cursor.unchanged(None, "claude", &removed),
            "a deleted transcript must reparse"
        );
    }

    #[test]
    fn cursor_keys_are_scoped_per_user_and_source() {
        let temp = tempfile::tempdir().expect("tempdir");
        let Seeded { dir: _, snap } = seeded(temp.path());
        let cursor = ScanCursor::new(temp.path().join("state"));

        cursor.store(Some("alice"), "claude", &snap).expect("store");
        assert!(cursor.unchanged(Some("alice"), "claude", &snap));
        assert!(
            !cursor.unchanged(Some("bob"), "claude", &snap),
            "another user's cursor must not satisfy this user's gate"
        );
        assert!(
            !cursor.unchanged(Some("alice"), "codex", &snap),
            "another source's cursor must not satisfy this source's gate"
        );
        assert!(
            !cursor.unchanged(None, "claude", &snap),
            "the local scope must not share the per-user cursor"
        );
    }

    #[test]
    fn malformed_cursor_file_reparses_and_is_overwritten() {
        let temp = tempfile::tempdir().expect("tempdir");
        let Seeded { dir: _, snap } = seeded(temp.path());
        let state = temp.path().join("state");
        let cursor = ScanCursor::new(state.clone());

        // Corrupt the cursor in place (the on-disk layout is part of the
        // contract: it is live state under /var/lib/ix-indexer on the fleet).
        let path = state.join("scan").join("local").join("claude.json");
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, b"not json").expect("corrupt");
        assert!(
            !cursor.unchanged(None, "claude", &snap),
            "a malformed cursor must reparse, not error or skip"
        );

        // The next successful run heals it.
        cursor.store(None, "claude", &snap).expect("store");
        assert!(cursor.unchanged(None, "claude", &snap));
        assert!(
            !path.with_extension("json.tmp").exists(),
            "the temp file must not linger"
        );
    }
}
