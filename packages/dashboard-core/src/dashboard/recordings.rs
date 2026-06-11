//! Durable, replayable recordings of the live board.
//!
//! The hub's Loro document is the recording: its oplog holds every change with a
//! millisecond timestamp, so one full snapshot replays the whole session. A
//! [`RecordingStore`] persists that snapshot to disk on an interval, so a
//! session survives an aggregator restart and can be opened or shared later,
//! and exposes the saved recordings for the HTTP routes in [`super::server`].
//!
//! Each aggregator run owns one recording file, `rec-<start-ms>.loro`, rewritten
//! in place as the document grows (one file per run, not a growing count), and
//! the store prunes to a bounded number of the most recent runs. The frontend
//! lists recordings, loads one into a detached document, and scrubs it with the
//! same timeline it uses for the live board.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinHandle;

use super::hub::Hub;
use crate::{Error, Result};

/// How many recent recordings to keep on disk. Older runs are pruned so a
/// long-lived host does not accumulate snapshots without bound.
const KEEP_RECORDINGS: usize = 50;

/// Recording filename prefix and extension. The id is the prefix plus the run's
/// start time in milliseconds; the extension marks a Loro snapshot.
const PREFIX: &str = "rec-";
const EXT: &str = "loro";

fn recording_err(message: impl Into<String>) -> Error {
    Error::Dashboard {
        message: format!("recordings: {}", message.into()),
    }
}

/// Metadata for one saved recording, as listed to the frontend.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RecordingInfo {
    /// The recording id (its filename stem), used in `GET /recording/<id>`.
    pub id: String,
    /// When the run started, parsed from the id: milliseconds since the epoch.
    pub started_ms: i64,
    /// When the snapshot was last written: milliseconds since the epoch.
    pub updated_ms: i64,
    /// The snapshot size in bytes.
    pub bytes: u64,
}

/// A directory of persisted recordings plus the helpers the recorder task and
/// the HTTP routes share.
pub struct RecordingStore {
    dir: PathBuf,
}

/// The result of starting a periodic recorder: the new recording's id and the
/// background task that keeps it current. The caller holds the task to tie its
/// lifetime to the dashboard.
pub struct Recorder {
    /// The id of the recording being written (its filename stem).
    pub id: String,
    /// The spawned task that refreshes the snapshot on each interval.
    pub task: JoinHandle<()>,
}

impl RecordingStore {
    /// Open (creating if needed) a store rooted at `dir`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dashboard`] when the directory cannot be created.
    pub fn new(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)
            .map_err(|source| recording_err(format!("create {}: {source}", dir.display())))?;
        // Recordings capture exec source, stdout, and stderr, so keep the
        // directory owner-only. This also stops another local user from planting
        // a symlink at a recording path before the atomic write below.
        restrict_dir(&dir);
        Ok(Self { dir })
    }

    /// Open the default store, under `$IX_DASH_RECORDINGS`, else
    /// `$XDG_STATE_HOME/ix-dash/recordings`, else `~/.local/state/...`, else a
    /// per-user `/tmp` fallback.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dashboard`] when the resolved directory cannot be created.
    pub fn open_default() -> Result<Self> {
        Self::new(default_recordings_dir())
    }

    /// The directory recordings are written to.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every saved recording, most recently started first.
    #[must_use]
    pub fn list(&self) -> Vec<RecordingInfo> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<RecordingInfo> = entries
            .flatten()
            .filter_map(|entry| Self::info(&entry.path()))
            .collect();
        out.sort_by_key(|info| std::cmp::Reverse(info.started_ms));
        out
    }

    /// The snapshot bytes for `id`, or `None` when it is unknown or the id is
    /// unsafe. The id must be a bare recording stem (no path separators), so a
    /// request can never read outside the store.
    #[must_use]
    pub fn load(&self, id: &str) -> Option<Vec<u8>> {
        let path = self.path_for(id)?;
        fs::read(&path).ok()
    }

    /// Write `bytes` as the snapshot for `id`, atomically (write a temp file then
    /// rename) so a reader never sees a half-written recording.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dashboard`] when the id is unsafe or the write fails.
    pub fn save(&self, id: &str, bytes: &[u8]) -> Result<()> {
        let path = self
            .path_for(id)
            .ok_or_else(|| recording_err(format!("unsafe recording id {id:?}")))?;
        let tmp = path.with_extension(format!("{EXT}.tmp"));
        write_private(&tmp, bytes)?;
        fs::rename(&tmp, &path)
            .map_err(|source| recording_err(format!("rename {}: {source}", path.display())))?;
        Ok(())
    }

    /// Delete all but the `keep` most-recent recordings.
    pub fn prune(&self, keep: usize) {
        let recordings = self.list();
        for stale in recordings.into_iter().skip(keep) {
            if let Some(path) = self.path_for(&stale.id) {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// Spawn a task on `runtime` that persists `hub`'s snapshot to a fresh
    /// recording every `interval`, returning the new recording's id and the task
    /// handle. Prunes old recordings first. The task writes a final snapshot path
    /// each tick (overwriting the same file), so the recording stays current.
    #[must_use]
    pub fn spawn_recorder(
        self: &Arc<Self>,
        hub: Arc<Hub>,
        interval: Duration,
        runtime: &tokio::runtime::Handle,
    ) -> Recorder {
        self.prune(KEEP_RECORDINGS.saturating_sub(1));
        let id = format!("{PREFIX}{}", now_ms());
        // Persist once up front so a session shorter than `interval` still
        // produces its advertised recording; the loop then refreshes it. A final
        // snapshot on graceful shutdown is the caller's job (it owns the hub).
        let initial = hub.export_snapshot();
        if !initial.is_empty() {
            let _ = self.save(&id, &initial);
        }
        let store = self.clone();
        let task_id = id.clone();
        let task = runtime.spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let bytes = hub.export_snapshot();
                if !bytes.is_empty() {
                    let _ = store.save(&task_id, &bytes);
                }
            }
        });
        Recorder { id, task }
    }

    /// Resolve a safe on-disk path for a recording id, or `None` when the id is
    /// not a bare recording stem (contains a separator, `..`, or the wrong shape).
    fn path_for(&self, id: &str) -> Option<PathBuf> {
        let valid = id.starts_with(PREFIX)
            && id.len() > PREFIX.len()
            && id[PREFIX.len()..].bytes().all(|byte| byte.is_ascii_digit());
        valid.then(|| self.dir.join(format!("{id}.{EXT}")))
    }

    /// Build the listing entry for a path, if it is a recording file.
    fn info(path: &Path) -> Option<RecordingInfo> {
        if path.extension().and_then(|ext| ext.to_str()) != Some(EXT) {
            return None;
        }
        let id = path.file_stem().and_then(|stem| stem.to_str())?.to_owned();
        // Reuse the id validation, which also parses the start time out of it.
        let started_ms = id.strip_prefix(PREFIX)?.parse().ok()?;
        let meta = fs::metadata(path).ok()?;
        let updated_ms = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
            .unwrap_or(started_ms);
        Some(RecordingInfo {
            id,
            started_ms,
            updated_ms,
            bytes: meta.len(),
        })
    }
}

/// Restrict the recordings directory to the owner (`0700`). Best-effort.
fn restrict_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
}

/// Write `bytes` to `path`, creating it owner-only (`0600`). Recordings hold
/// captured source and output, so they must not be world-readable even briefly.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| recording_err(format!("open {}: {source}", path.display())))?;
    file.write_all(bytes)
        .map_err(|source| recording_err(format!("write {}: {source}", path.display())))?;
    Ok(())
}

/// Resolve the recordings directory: `$IX_DASH_RECORDINGS`, else
/// `$XDG_STATE_HOME/ix-dash/recordings`, else `~/.local/state/ix-dash/recordings`,
/// else `/tmp/ix-dash-recordings-<user>`.
fn default_recordings_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("IX_DASH_RECORDINGS") {
        return PathBuf::from(dir);
    }
    if let Some(state) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(state).join("ix-dash/recordings");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".local/state/ix-dash/recordings");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "shared".to_owned());
    PathBuf::from(format!("/tmp/ix-dash-recordings-{user}"))
}

/// Milliseconds since the Unix epoch, saturating rather than panicking.
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> RecordingStore {
        use std::sync::atomic::{AtomicU64, Ordering};
        // A per-process counter, not the clock: parallel tests can share a
        // millisecond, which would put two stores in one directory.
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ix-dash-rec-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        RecordingStore::new(dir).expect("store")
    }

    /// A round-trip through disk returns the same bytes, and the listing reports
    /// the recording with its parsed start time.
    #[test]
    fn save_load_list_round_trip() {
        let store = temp_store();
        let id = format!("{PREFIX}1700000000000");
        store.save(&id, b"snapshot-bytes").unwrap();
        assert_eq!(
            store.load(&id).as_deref(),
            Some(b"snapshot-bytes".as_slice())
        );

        let list = store.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, id);
        assert_eq!(list[0].started_ms, 1_700_000_000_000);
        assert_eq!(list[0].bytes, b"snapshot-bytes".len() as u64);

        let _ = fs::remove_dir_all(store.dir());
    }

    /// An id that escapes the store (path traversal, separators, wrong shape) is
    /// refused by both load and save, so a request can never reach another file.
    #[test]
    fn rejects_unsafe_ids() {
        let store = temp_store();
        for bad in [
            "../secret",
            "rec-../x",
            "rec-abc",
            "evil",
            "rec-",
            "rec-1/2",
        ] {
            assert!(store.load(bad).is_none(), "load must reject {bad:?}");
            assert!(store.save(bad, b"x").is_err(), "save must reject {bad:?}");
        }
        let _ = fs::remove_dir_all(store.dir());
    }

    /// Pruning keeps the most-recent recordings and drops the rest.
    #[test]
    fn prune_keeps_most_recent() {
        let store = temp_store();
        for start in [1000_u64, 3000, 2000] {
            store.save(&format!("{PREFIX}{start}"), b"x").unwrap();
        }
        store.prune(2);
        let kept: Vec<i64> = store
            .list()
            .into_iter()
            .map(|info| info.started_ms)
            .collect();
        assert_eq!(kept, vec![3000, 2000], "newest two survive, sorted desc");
        let _ = fs::remove_dir_all(store.dir());
    }
}
