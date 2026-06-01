//! Durable history of [`Run`]s, keyed by `(machine_id, git_commit,
//! timestamp_unix)`.
//!
//! A store is the framework's memory: every run is appended, and the comparator
//! reads back the previous run on the same machine (or a pinned commit's run) as
//! its baseline. The trait is small on purpose — append a run, list runs for a
//! `(suite, bench)` — so a new backend is a few methods, not a rewrite.
//!
//! # Why an orphan git branch is the default
//!
//! [`GitBranchStore`] commits JSONL to an orphan `bench-history` branch in the
//! same repository. This is the default because it is *durable, versioned, and
//! shared* without new infrastructure: anyone with the repo has the full
//! history, a CI job can `git push` results, and the data is content-addressed
//! and auditable like any other commit. Critically, an orphan branch shares no
//! ancestry with `main`, so the growing JSONL never enters `main`'s tree or
//! working copy — a `git checkout main` never materializes a single history
//! file. The alternative ([`LocalDirStore`]) keeps JSONL in a directory for
//! laptop iteration and tests, where committing every run would be noise. A
//! future object-store backend implements the same trait without touching the
//! harnesses or the comparator.

use std::path::{Path, PathBuf};
use std::process::Command;

use snafu::{ResultExt, ensure};

use crate::error::{self};
use crate::schema::Run;

/// A durable, append-only history of runs.
///
/// Implementations must make [`append`](HistoryStore::append) durable before it
/// returns, and [`runs_for`](HistoryStore::runs_for) must return the matching
/// runs in ascending `timestamp_unix` order so callers can take the last as the
/// most recent.
pub trait HistoryStore {
    /// Append one run to the store, making it durable before returning.
    ///
    /// # Errors
    ///
    /// Returns an error when the backing storage cannot be written.
    fn append(&self, run: &Run) -> crate::Result<()>;

    /// All runs recorded for a `(suite, bench)`, ascending by timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when the backing storage cannot be read or a stored
    /// record does not parse.
    fn runs_for(&self, suite: &str, bench: &str) -> crate::Result<Vec<Run>>;

    /// The most recent run for `(suite, bench)` on `machine_id`, the default
    /// comparison baseline. `None` when this machine has no prior run.
    ///
    /// # Errors
    ///
    /// Propagates a read failure from [`runs_for`](HistoryStore::runs_for).
    fn previous_run(&self, suite: &str, bench: &str, machine_id: &str) -> crate::Result<Option<Run>> {
        let mut runs = self.runs_for(suite, bench)?;
        runs.retain(|run| run.machine_id == machine_id);
        Ok(runs.pop())
    }

    /// The most recent run for `(suite, bench)` on `machine_id` at a pinned
    /// `git_commit`, used by `--baseline <commit>`. `None` when no such run
    /// exists.
    ///
    /// # Errors
    ///
    /// Propagates a read failure from [`runs_for`](HistoryStore::runs_for).
    fn run_at_commit(&self, suite: &str, bench: &str, machine_id: &str, git_commit: &str) -> crate::Result<Option<Run>> {
        let mut runs = self.runs_for(suite, bench)?;
        runs.retain(|run| run.machine_id == machine_id && run.git_commit == git_commit);
        Ok(runs.pop())
    }
}

/// Append `run` as one JSONL line to `writer`. Shared by the file-backed stores
/// so the on-disk format has a single source of truth.
fn run_to_jsonl(run: &Run) -> crate::Result<String> {
    let mut line = serde_json::to_string(run).context(error::SerializeSnafu)?;
    line.push('\n');
    Ok(line)
}

/// Parse JSONL `contents` for `(suite, bench)`, ascending by timestamp.
fn parse_jsonl(contents: &str, source: &Path, suite: &str, bench: &str) -> crate::Result<Vec<Run>> {
    let mut runs = Vec::new();
    for line in contents.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let run: Run = serde_json::from_str(line).with_context(|_| error::StoreParseSnafu { path: source.to_owned() })?;
        if run.suite == suite && run.bench == bench {
            runs.push(run);
        }
    }
    runs.sort_by_key(|run| run.timestamp_unix);
    Ok(runs)
}

/// A history store backed by a single JSONL file per directory.
///
/// All runs (every suite, every bench, every machine) append to one
/// `history.jsonl`; `runs_for` filters by `(suite, bench)`. One file keeps the
/// laptop case trivial and makes the git-branch store a thin wrapper over the
/// same format.
pub struct LocalDirStore {
    path: PathBuf,
}

impl LocalDirStore {
    /// Create a store writing to `<dir>/history.jsonl`, creating `dir` if
    /// needed.
    ///
    /// # Errors
    ///
    /// Returns an error when `dir` cannot be created.
    pub fn new(dir: impl AsRef<Path>) -> crate::Result<Self> {
        let dir = dir.as_ref();
        std::fs::create_dir_all(dir).with_context(|_| error::StoreWriteSnafu { path: dir.to_owned() })?;
        Ok(Self {
            path: dir.join("history.jsonl"),
        })
    }
}

impl HistoryStore for LocalDirStore {
    fn append(&self, run: &Run) -> crate::Result<()> {
        use std::io::Write;

        let line = run_to_jsonl(run)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|_| error::StoreWriteSnafu { path: self.path.clone() })?;
        file.write_all(line.as_bytes()).with_context(|_| error::StoreWriteSnafu { path: self.path.clone() })?;
        // fsync so a crash right after `append` cannot lose a just-recorded run.
        file.sync_all().with_context(|_| error::StoreWriteSnafu { path: self.path.clone() })?;
        Ok(())
    }

    fn runs_for(&self, suite: &str, bench: &str) -> crate::Result<Vec<Run>> {
        let contents = match std::fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err).with_context(|_| error::StoreReadSnafu { path: self.path.clone() }),
        };
        parse_jsonl(&contents, &self.path, suite, bench)
    }
}

/// A history store backed by an orphan git branch in a repository.
///
/// Runs are appended to a single `history.jsonl` blob held on the `branch`
/// (default `bench-history`), which shares no history with `main`. Each append
/// is one commit built with plumbing (`hash-object` to write the blob, `mktree`
/// to build the one-entry tree, `commit-tree`, `update-ref`) so the working tree
/// and the index are never touched. This makes the store usable from a dirty
/// checkout mid-bench, and a CI job can `git push origin <branch>` to share
/// results.
pub struct GitBranchStore {
    repo: PathBuf,
    branch: String,
    blob_path: String,
}

impl GitBranchStore {
    /// Default branch name for the history store.
    pub const DEFAULT_BRANCH: &'static str = "bench-history";

    /// `git update-ref` old-value meaning "this ref must not already exist".
    /// The empty string is hash-agnostic (works for sha1 and sha256 repos),
    /// unlike a fixed-length all-zeros oid; used as the compare-and-swap old
    /// value when creating the branch for the first time.
    const REF_MUST_NOT_EXIST: &'static str = "";

    /// Open the store for the git repository at `repo`, using `branch`.
    pub fn new(repo: impl Into<PathBuf>, branch: impl Into<String>) -> Self {
        Self {
            repo: repo.into(),
            branch: branch.into(),
            blob_path: "history.jsonl".to_owned(),
        }
    }

    /// Run a `git` plumbing command in the repo, returning captured stdout.
    fn git(&self, args: &[&str], stdin: Option<&[u8]>) -> crate::Result<Vec<u8>> {
        use std::io::Write;

        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .stdin(if stdin.is_some() {
                std::process::Stdio::piped()
            } else {
                std::process::Stdio::null()
            });

        let mut child = command.spawn().map_err(|err| error::Error::Git {
            operation: args.first().copied().unwrap_or("?").to_owned(),
            detail: err.to_string(),
        })?;

        if let (Some(bytes), Some(mut sink)) = (stdin, child.stdin.take()) {
            sink.write_all(bytes).map_err(|err| error::Error::Git {
                operation: args.first().copied().unwrap_or("?").to_owned(),
                detail: err.to_string(),
            })?;
        }

        let output = child.wait_with_output().map_err(|err| error::Error::Git {
            operation: args.first().copied().unwrap_or("?").to_owned(),
            detail: err.to_string(),
        })?;

        ensure!(
            output.status.success(),
            error::GitSnafu {
                operation: args.first().copied().unwrap_or("?").to_owned(),
                detail: String::from_utf8_lossy(&output.stderr).into_owned(),
            }
        );
        Ok(output.stdout)
    }

    /// Like [`git`](Self::git) but returns trimmed UTF-8 stdout as a `String` —
    /// the shape every object-id-producing plumbing call here wants.
    fn git_str(&self, args: &[&str], stdin: Option<&[u8]>) -> crate::Result<String> {
        Ok(String::from_utf8_lossy(&self.git(args, stdin)?).trim().to_owned())
    }

    /// The commit the branch currently points at, or `None` when the branch does
    /// not exist yet.
    fn branch_tip(&self) -> crate::Result<Option<String>> {
        let refname = format!("refs/heads/{}", self.branch);
        // `rev-parse --verify --quiet` exits non-zero when the ref is missing;
        // treat that as "no branch yet" rather than an error.
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.repo).args(["rev-parse", "--verify", "--quiet", &refname]);
        let output = command.output().map_err(|err| error::Error::Git {
            operation: "rev-parse".to_owned(),
            detail: err.to_string(),
        })?;
        if !output.status.success() {
            return Ok(None);
        }
        Ok(Some(String::from_utf8_lossy(&output.stdout).trim().to_owned()))
    }

    /// Read the current `history.jsonl` blob from the branch tip, or empty when
    /// the branch or blob does not exist.
    fn read_blob(&self) -> crate::Result<String> {
        let Some(tip) = self.branch_tip()? else {
            return Ok(String::new());
        };
        let spec = format!("{tip}:{}", self.blob_path);
        // `show` failing simply means the blob is not on this commit yet; fall
        // back to empty rather than propagating, so a first-ever append works.
        Ok(self.git(&["show", &spec], None).map_or_else(
            |_| String::new(),
            |bytes| String::from_utf8_lossy(&bytes).into_owned(),
        ))
    }
}

impl HistoryStore for GitBranchStore {
    fn append(&self, run: &Run) -> crate::Result<()> {
        let mut contents = self.read_blob()?;
        contents.push_str(&run_to_jsonl(run)?);

        // Write the new blob, then build a one-entry tree with `mktree`, which
        // reads `<mode> <type> <sha>\t<path>` lines on stdin and writes the tree
        // object. No index, no working-tree touch — pure plumbing.
        let blob = self.git_str(&["hash-object", "-w", "--stdin"], Some(contents.as_bytes()))?;
        let tree_entry = format!("100644 blob {blob}\t{}\n", self.blob_path);
        let tree = self.git_str(&["mktree"], Some(tree_entry.as_bytes()))?;

        let parent = self.branch_tip()?;
        let message = format!("bench: {}/{} on {} @ {}", run.suite, run.bench, run.machine_id, run.git_commit);
        let mut commit_args = vec!["commit-tree".to_owned(), tree, "-m".to_owned(), message];
        if let Some(parent) = &parent {
            commit_args.push("-p".to_owned());
            commit_args.push(parent.clone());
        }
        let commit_args_ref: Vec<&str> = commit_args.iter().map(String::as_str).collect();
        let commit = self.git_str(&commit_args_ref, None)?;

        // Compare-and-swap the ref against the tip we read, so a concurrent
        // append (another `indexbench run` on the same branch) fails loudly
        // instead of silently overwriting — and thus dropping — the other run.
        // The expected old value is the empty oid when the branch did not exist.
        let refname = format!("refs/heads/{}", self.branch);
        let expected = parent.as_deref().unwrap_or(Self::REF_MUST_NOT_EXIST);
        self.git(&["update-ref", &refname, &commit, expected], None)?;
        Ok(())
    }

    fn runs_for(&self, suite: &str, bench: &str) -> crate::Result<Vec<Run>> {
        let contents = self.read_blob()?;
        let source = self.repo.join(format!("{}:{}", self.branch, self.blob_path));
        parse_jsonl(&contents, &source, suite, bench)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Metric;

    fn sample_run(timestamp: i64, commit: &str) -> Run {
        Run {
            suite: "self-demo".to_owned(),
            bench: "fib".to_owned(),
            metrics: vec![Metric::deterministic("allocations", 3.0, "count", true)],
            machine_id: "machine-a".to_owned(),
            git_commit: commit.to_owned(),
            git_dirty: false,
            timestamp_unix: timestamp,
        }
    }

    #[test]
    fn local_store_round_trips_and_orders_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalDirStore::new(dir.path()).expect("store");

        store.append(&sample_run(200, "bbb")).expect("append later");
        store.append(&sample_run(100, "aaa")).expect("append earlier");

        let runs = store.runs_for("self-demo", "fib").expect("read");
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].timestamp_unix, 100, "runs must come back ascending");
        assert_eq!(runs[1].timestamp_unix, 200);
    }

    #[test]
    fn local_store_previous_run_is_latest_for_machine() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalDirStore::new(dir.path()).expect("store");
        store.append(&sample_run(100, "aaa")).expect("append");
        store.append(&sample_run(200, "bbb")).expect("append");

        let previous = store.previous_run("self-demo", "fib", "machine-a").expect("previous");
        assert_eq!(previous.map(|r| r.timestamp_unix), Some(200));

        let other = store.previous_run("self-demo", "fib", "machine-z").expect("previous");
        assert!(other.is_none(), "a machine with no runs has no baseline");
    }

    #[test]
    fn local_store_run_at_commit_pins_baseline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalDirStore::new(dir.path()).expect("store");
        store.append(&sample_run(100, "aaa")).expect("append");
        store.append(&sample_run(200, "bbb")).expect("append");

        let pinned = store.run_at_commit("self-demo", "fib", "machine-a", "aaa").expect("pinned");
        assert_eq!(pinned.map(|r| r.git_commit), Some("aaa".to_owned()));
    }

    #[test]
    fn local_store_missing_file_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalDirStore::new(dir.path()).expect("store");
        assert!(store.runs_for("self-demo", "fib").expect("read").is_empty());
    }

    /// Append two runs through the real git plumbing (the `mktree` staging path)
    /// and read them back. This is the regression guard for the staging bug: an
    /// invalid `mktree`/`update-index` invocation fails `append` outright, so a
    /// successful round-trip proves the default store actually records.
    ///
    /// Skips when `git` is not on `PATH` (some minimal sandboxes), so the suite
    /// stays green there rather than failing on a missing tool.
    #[test]
    fn git_store_round_trips_through_mktree() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let repo = dir.path();
        let run_git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(args)
                .status()
                .expect("git runs");
            assert!(status.success(), "git {args:?} succeeds");
        };
        run_git(&["init", "--quiet"]);
        // Repo-local identity so `commit-tree` has an author/committer without
        // depending on the sandbox's global git config.
        run_git(&["config", "user.email", "bench@example.com"]);
        run_git(&["config", "user.name", "bench"]);

        let store = GitBranchStore::new(repo.to_path_buf(), GitBranchStore::DEFAULT_BRANCH.to_owned());
        store.append(&sample_run(100, "aaa")).expect("append first");
        store.append(&sample_run(200, "bbb")).expect("append second");

        let runs = store.runs_for("self-demo", "fib").expect("read back");
        assert_eq!(runs.len(), 2, "both runs are recorded on the branch");
        assert_eq!(runs[0].timestamp_unix, 100, "runs come back ascending");
        assert_eq!(runs[1].git_commit, "bbb");
    }
}
