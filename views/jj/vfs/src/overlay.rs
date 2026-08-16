// Copyright 2026 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! An additive writable layer over a revision's tree.
//!
//! Build tooling cannot run against a read-only mount. `bun install` wants a
//! `node_modules`, cargo wants a `target`, and both fail at the first `mkdir`.
//! This module is what lets them run: writes land in a real directory on the
//! host, reads resolve that directory first and the revision second, and the
//! object store is never written to.
//!
//! # The one invariant
//!
//! **Nothing done through the mount can cost you anything the revision holds.**
//! The lower layer is immutable and content-addressed and is never written to,
//! so discarding the scratch directory restores the revision exactly, whatever
//! happened in between. That is the property worth defending, and everything
//! below is a consequence of it rather than a second rule.
//!
//! A tracked *file or symlink* can be shadowed, and it can be deleted. Writing
//! to one copies it up and the name resolves to the copy; deleting one records
//! a whiteout, and the name stops resolving until something creates it again.
//! A tracked *directory* can only be added to. Removing one, and renaming one
//! away, are refused with `EROFS`.
//!
//! Directories are the exception because hiding one means hiding a subtree:
//! opaque directories, readdir subtraction below the whiteout, and the class
//! of bug where files under a resurrected directory come back in the wrong
//! state. Files need none of that. A whiteout on a file is one name in one
//! set, tested at the two points that turn a name into an entry, and the
//! entire cost of the feature is those two tests plus a log to persist the set.
//!
//! # What that costs
//!
//! `rm -rf` of a directory the revision contains still fails, at the `rmdir`
//! rather than at the first file: the files under it go, the directory itself
//! does not. `git clean -xfd` fails for the same reason. `rm -rf node_modules`
//! works, because `node_modules` is not in the revision at all.
//!
//! A directory has no POSIX mode bit for "you may add a name here but not
//! remove one", so a tracked directory reports `0o755` and a caller that
//! inspects the mode before unlinking is told yes and then gets `EROFS` from
//! the syscall. Reporting `0o555` instead would block the `mkdir` this module
//! exists to allow, so no mode is honest about both.
//!
//! # A whiteout belongs to a revision, not to a path
//!
//! Deleting `bun.lock` says something about the file the revision had at that
//! name. It says nothing about whatever a *different* revision has there, so a
//! scratch layer remounted at a different revision starts with no whiteouts
//! and every tracked name back. Untracked scratch content is unaffected and
//! still persists across the remount, which is the whole reason the scratch
//! layer outlives one mount.
//!
//! That is the safe direction to be wrong in: changing revision can only make
//! the mount show more names, never fewer. The alternative, carrying whiteouts
//! across revisions, hides files at a revision where the caller never asked
//! for them to be hidden.
//!
//! The set is persisted in a log at [`WHITEOUT_NAME`] inside the scratch
//! directory, whose first line names the revision it belongs to. A log whose
//! revision does not match the tree being mounted is discarded on open rather
//! than migrated.
//!
//! # Copy-up, and why the narrower rule was not enough
//!
//! An earlier cut of this allowed writes only to paths the revision does not
//! contain. It is smaller, and it does not work. `bun install` in a workspace
//! package resolves up to the workspace root and rewrites the root lockfile,
//! which is tracked; so is `Cargo.lock`, so is `uv.lock`. Under the narrower
//! rule the reported `mkdir node_modules` failure is fixed and the next step
//! fails the same way, having already half-populated a tree.
//!
//! The narrow rule also does not help tools that only ever touch untracked
//! paths, because well-behaved writers do not open the target: they write a
//! temp file and rename it into place. The temp name is untracked and allowed,
//! and then the rename onto the tracked name is refused. So renaming *onto* a
//! tracked path has to work, and once it does the tracked name already
//! resolves to upper content, which is copy-up by another route. Refusing the
//! direct write while allowing the rename would be an arbitrary line.
//!
//! # Storage
//!
//! The upper layer is an ordinary directory tree mirroring repo paths, so it
//! can be inspected, backed up and deleted with the tools anyone already has.
//! Where it lives and how long it lives is deliberately not this crate's
//! decision; a caller hands over a directory. See `jj fs mount`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::io::Write as _;
use std::num::NonZeroUsize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;

use clru::CLruCache;
use jj_lib::lock::FileLock;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::repo_path::RepoPathComponent;

use crate::snapshot::Attributes;
use crate::snapshot::EntryKind;
use crate::snapshot::ROOT_INODE;
use crate::snapshot::SnapshotError;
use crate::snapshot::TreeEntry;
use crate::snapshot::TreeSnapshot;
use crate::stats::Op;
use crate::stats::Stats;
use crate::stats::Timer;
use crate::sys;

type Result<T> = std::result::Result<T, SnapshotError>;

/// First inode number handed out to a path that exists only in the upper layer.
///
/// The two layers allocate inodes independently, so they need disjoint ranges.
/// A path the revision contains keeps the number [`TreeSnapshot`] gave it even
/// after it is copied up, because an NFS file handle is built from the inode
/// and changing it mid-write would hand the client `ESTALE` on its own file.
///
/// 2^48 rather than the high bit: setting bit 63 produces numbers that read as
/// negative to anything storing an inode in a signed 64-bit column, which is
/// common enough to avoid for no benefit. The lower allocator would have to
/// intern 2.8e14 paths to reach this and each one is a heap-allocated struct,
/// so the collision is not reachable on a real machine. It is checked anyway;
/// see [`OverlayTree::resolve`].
const UPPER_INODE_BASE: u64 = 1 << 48;

/// Name of the lock file inside the upper directory.
///
/// Inside rather than beside it, so the lock travels with the thing it
/// protects. Filtered out of listings by [`OverlayTree::upper_entries`].
const LOCK_NAME: &str = ".jj-overlay-lock";

/// Name of the whiteout log inside the upper directory.
///
/// Beside the lock, and filtered out of listings for the same reason.
pub const WHITEOUT_NAME: &str = ".jj-overlay-whiteouts";

/// First line of a whiteout log, followed by a space and the revision key.
///
/// A log that does not start with exactly this is not one of ours, or is one
/// from a format we no longer read, and is discarded rather than parsed.
const WHITEOUT_FORMAT: &str = "jj-overlay-whiteouts 1";

/// Where a whiteout log is rewritten before being renamed over the real one.
///
/// Named rather than derived, so that the listing filter can exclude it by
/// exact name: a process killed mid-rewrite leaves this behind, and a scratch
/// file appearing in a listing of the revision is exactly what the filter is
/// there to prevent.
const WHITEOUT_TEMP_NAME: &str = ".jj-overlay-whiteouts.new";

/// How many superseded records a whiteout log carries before it is rewritten.
///
/// The log is append-only so that hiding a name is one small write rather than
/// a rewrite of every name hidden so far, which would be quadratic in a `rm`
/// over many files. Compaction keeps it from growing without bound when a
/// caller deletes and recreates the same name repeatedly, which is exactly
/// what `bun install` does to a lockfile. The floor keeps a scratch layer with
/// two whiteouts from rewriting its log on every third operation.
const WHITEOUT_COMPACT_FLOOR: usize = 64;

/// Prefix macOS uses for an AppleDouble sidecar file.
///
/// macOS stamps `com.apple.provenance` on files it writes, NFSv3 has no
/// extended attributes, so the client materializes every one as a 4.1 KB
/// `._name` file beside the real one. Measured on one `bun install` against the
/// ix workspace: 39,003 sidecars for 38,517 real entries, about 160 MB, and all
/// of them visible in listings through the mount.
///
/// So a write to one of these names is accepted, counted, and thrown away, and
/// the name never appears in a listing. This is a deliberate macOS
/// accommodation and not a general policy: the bytes being dropped are a
/// transport artifact carrying an xattr that has no representation in a jj
/// tree, not anything a caller put there on purpose.
///
/// The proper fix is `namedattr`, which `mount_nfs` supports and documents as
/// "for NFSv4 mounts". nfs3_server speaks NFSv3 only, so it is unreachable
/// without changing transports. If we ever move to a transport with native
/// xattrs, this whole accommodation should be deleted rather than carried.
const APPLEDOUBLE_PREFIX: &str = "._";

/// Whether a name is an AppleDouble sidecar rather than a caller's file.
fn is_sidecar(name: &str) -> bool {
    name.starts_with(APPLEDOUBLE_PREFIX)
}

/// Whether a name is one of the scratch layer's own files.
///
/// These live in the upper directory because they have to travel with the
/// thing they describe, which means they sit in the same namespace the mount
/// serves. Nothing reaches them through the mount: they are not listed, they
/// do not resolve, and an attempt to create or delete one is refused. Serving
/// them would put names in the listing that belong to no revision, and letting
/// a caller unlink one would break the lock or lose every whiteout.
///
/// The cost is that a revision containing a file at one of these three names
/// cannot show it. Deemed better than the alternative, which is a `rm -rf` of
/// the mount taking the lock with it.
fn is_scratch_name(name: &str) -> bool {
    matches!(name, LOCK_NAME | WHITEOUT_NAME | WHITEOUT_TEMP_NAME)
}

/// Mode bits reported for a writable mount.
///
/// A read-only mount reports `0o444` and `0o555`, which is accurate there. A
/// writable mount reporting the same would have the kernel refuse every
/// `mkdir` and every open-for-write before the server was asked, so these are
/// the modes that make the feature work at all. They are honest for files,
/// since copy-up means a tracked file really can be written. They are not
/// honest for directories; see the module docs.
const MODE_DIR_RW: u32 = 0o755;
const MODE_FILE_RW: u32 = 0o644;
const MODE_EXEC_RW: u32 = 0o755;

/// Mode bits reported for a read-only mount.
const MODE_DIR_RO: u32 = 0o555;
const MODE_FILE_RO: u32 = 0o444;
const MODE_EXEC_RO: u32 = 0o555;

/// A symlink's own mode is meaningless; every filesystem reports 0777.
const MODE_SYMLINK: u32 = 0o777;

/// Which layer answers for a path.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Layer {
    /// Served from the revision.
    Lower,
    /// Served from the upper directory, at the given host path.
    Upper(PathBuf),
    /// An AppleDouble sidecar. Backed by nothing; the value is the size the
    /// client has been told it has.
    Discarded(u64),
}

/// How many files the writable layer keeps open at once.
///
/// A write that has to `open` first pays for the open on every call. Measured
/// on one `bun install`: 290,879 writes at 32.8 us each, 9.5 seconds, where the
/// write itself is a small fraction of that. Holding the descriptor turns the
/// second and subsequent writes to a file into one `pwrite`.
///
/// Bounded because descriptors are a per-process limit and a build touches tens
/// of thousands of files. Eviction closes the file, which is correct at any
/// point: nothing is buffered in the handle, so a closed one is reopened on
/// demand and no data is at risk.
const OPEN_HANDLE_LIMIT: usize = 512;

/// What the writable layer knows about an entry without asking the filesystem.
///
/// Sound because this process is the only writer: the exclusive lock keeps
/// another server out, and the only route to these files is through this mount.
/// Every mutation below updates this in the same breath as the write it
/// describes. If somebody edits the scratch directory by hand while a mount is
/// live, this goes stale, which is the same assumption `shadowed` already
/// makes and the reason the scratch directory is not a place to work by hand.
#[derive(Clone, Copy, Debug)]
struct CachedAttributes {
    kind: EntryKind,
    size: u64,
    mtime: SystemTime,
}

/// Everything the upper layer tracks, behind one lock.
///
/// One mutex rather than three because every mutation touches at least two of
/// these and the critical sections are microseconds of hashing.
#[derive(Debug, Default)]
struct UpperState {
    /// Repo path of each upper-only inode, indexed by `inode -
    /// UPPER_INODE_BASE`.
    paths: Vec<RepoPathBuf>,
    /// The reverse, for upper-only paths.
    by_path: HashMap<RepoPathBuf, u64>,
    /// Whether a path the revision contains also has something in the upper
    /// layer. Absent means "not looked at yet"; the answer is discovered with
    /// one `lstat` and then maintained, which is sound because the exclusive
    /// lock makes this process the only writer.
    shadowed: HashMap<RepoPathBuf, bool>,
    /// Attributes of entries in the writable layer, by inode.
    ///
    /// This exists because `getattr` was 3,397,989 calls at 2.9 us, 9.9 seconds
    /// and 62.5% of all traffic, and every one of them was an `lstat` for
    /// something we had just written and therefore already knew.
    attributes: HashMap<u64, CachedAttributes>,
    /// AppleDouble sidecars the client believes it has created, and the size it
    /// believes each one has.
    ///
    /// Nothing is stored for them. They are reported as ordinary empty-ish
    /// files so the client's own bookkeeping stays consistent, and they are
    /// never listed. See [`APPLEDOUBLE_PREFIX`].
    discarded: HashMap<RepoPathBuf, u64>,
}

/// Tracked names this scratch layer has deleted, and the log that outlives it.
///
/// Files and symlinks only. Removing a tracked directory is refused, so no
/// entry here names one, which is what keeps the two subtraction points a set
/// membership test rather than a prefix walk.
///
/// The log is not flushed to disk synchronously and is not fsynced. A crash
/// can therefore lose the last few whiteouts, which resurrects names the
/// caller deleted. That is the same direction of error as remounting at a new
/// revision, and the alternative is an fsync on the unlink path of a
/// filesystem whose entire purpose is to be fast at bulk file operations.
#[derive(Debug)]
struct Whiteouts {
    hidden: HashSet<RepoPathBuf>,
    /// Records written to the log, live and superseded together.
    records: usize,
    log: PathBuf,
}

impl Whiteouts {
    /// Reads the log belonging to `revision`, or starts an empty one.
    ///
    /// A log written against a different revision is truncated here rather
    /// than left in place, so that the file on disk always describes the mount
    /// that is running.
    fn load(root: &Path, revision: &str) -> Result<Self> {
        let log = root.join(WHITEOUT_NAME);
        let header = format!("{WHITEOUT_FORMAT} {revision}");
        let existing = match fs::read_to_string(&log) {
            Ok(text) => Some(text),
            Err(err) if err.kind() == io::ErrorKind::NotFound => None,
            Err(err) => return Err(io_error(&log, err)),
        };
        let mut whiteouts = Self {
            hidden: HashSet::new(),
            records: 0,
            log,
        };
        let matching = existing
            .as_deref()
            .filter(|text| text.lines().next() == Some(header.as_str()));
        let Some(text) = matching else {
            whiteouts.rewrite(revision)?;
            return Ok(whiteouts);
        };
        for line in text.lines().skip(1) {
            let (hide, rest) = match (line.strip_prefix("- "), line.strip_prefix("+ ")) {
                (Some(rest), _) => (true, rest),
                (_, Some(rest)) => (false, rest),
                _ => {
                    // One unreadable line does not justify throwing away the
                    // rest, and the worst case of skipping it is a name that
                    // reappears.
                    tracing::warn!(line, "skipping unreadable whiteout record");
                    continue;
                }
            };
            let Some(path) =
                unescape(rest).and_then(|text| RepoPathBuf::from_internal_string(text).ok())
            else {
                tracing::warn!(line, "skipping whiteout record that is not a repo path");
                continue;
            };
            whiteouts.records += 1;
            if hide {
                whiteouts.hidden.insert(path);
            } else {
                whiteouts.hidden.remove(&path);
            }
        }
        Ok(whiteouts)
    }

    fn contains(&self, path: &RepoPath) -> bool {
        self.hidden.contains(path)
    }

    /// Hides a tracked name. Returns whether this changed anything.
    fn hide(&mut self, path: &RepoPath, revision: &str) -> Result<bool> {
        if !self.hidden.insert(path.to_owned()) {
            return Ok(false);
        }
        self.append("-", path, revision)?;
        Ok(true)
    }

    /// Un-hides a name, because something created it again.
    fn reveal(&mut self, path: &RepoPath, revision: &str) -> Result<bool> {
        if !self.hidden.remove(path) {
            return Ok(false);
        }
        self.append("+", path, revision)?;
        Ok(true)
    }

    fn append(&mut self, sign: &str, path: &RepoPath, revision: &str) -> Result<()> {
        let record = format!("{sign} {}\n", escape(path));
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&self.log)
            .map_err(|err| io_error(&self.log, err))?;
        file.write_all(record.as_bytes())
            .map_err(|err| io_error(&self.log, err))?;
        self.records += 1;
        if self.records > 2 * self.hidden.len() + WHITEOUT_COMPACT_FLOOR {
            self.rewrite(revision)?;
        }
        Ok(())
    }

    /// Writes the live set out as a fresh log.
    ///
    /// Through a temporary and renamed into place, because a log truncated
    /// half way through a rewrite would resurrect an arbitrary subset of the
    /// names it holds.
    fn rewrite(&mut self, revision: &str) -> Result<()> {
        let mut text = format!("{WHITEOUT_FORMAT} {revision}\n");
        // Sorted so that two rewrites of the same set produce the same file,
        // which is what makes the log worth reading by eye and diffing.
        let mut live: Vec<&RepoPathBuf> = self.hidden.iter().collect();
        live.sort_unstable();
        for path in &live {
            text.push_str("- ");
            text.push_str(&escape(path));
            text.push('\n');
        }
        let temporary = self.log.with_file_name(WHITEOUT_TEMP_NAME);
        fs::write(&temporary, text).map_err(|err| io_error(&temporary, err))?;
        fs::rename(&temporary, &self.log).map_err(|err| io_error(&self.log, err))?;
        self.records = self.hidden.len();
        Ok(())
    }
}

/// A repo path as one line of a whiteout log.
///
/// A filesystem name may contain a newline, which would otherwise end the
/// record early and hide a path nobody deleted. A carriage return is escaped
/// for a quieter reason: the log is read back with `str::lines`, which strips
/// a trailing `\r` as part of a `\r\n` pair, so a name ending in one would come
/// back a byte shorter and hide its neighbor instead.
fn escape(path: &RepoPath) -> String {
    path.as_internal_file_string()
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// The inverse of [`escape`]. `None` for a record that is not one of ours.
fn unescape(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            out.push(character);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            '\\' => out.push('\\'),
            _ => return None,
        }
    }
    Some(out)
}

/// A writable layer stacked on a revision.
pub struct Overlay {
    root: PathBuf,
    /// Held for the life of the overlay. Two servers sharing one upper
    /// directory would be two writers with nothing between them, and that pair
    /// is reachable today rather than hypothetical: a `jj fs mount` can outlive
    /// its own mountpoint, so remounting the same path produces it.
    _lock: FileLock,
    state: Mutex<UpperState>,
    /// Open write handles, most recently used first. A separate lock from
    /// `state` so that a write in progress does not block a lookup.
    handles: Mutex<CLruCache<u64, Arc<fs::File>>>,
    /// Tracked names the caller has deleted. Its own lock because a lookup
    /// reads it on every miss in the upper layer, and a write to it is rare.
    whiteouts: Mutex<Whiteouts>,
    /// The revision this scratch layer is bound to, written into the whiteout
    /// log so that a later mount can tell whether the log is still about the
    /// tree it is looking at.
    revision: String,
}

// Hand-written because `jj_lib::lock::FileLock` is not `Debug` and the useful
// part of an overlay is where it lives anyway.
impl std::fmt::Debug for Overlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Overlay").field("root", &self.root).finish()
    }
}

impl Overlay {
    /// Takes exclusive ownership of `root` as an upper layer, creating the
    /// directory if it does not exist.
    ///
    /// Fails rather than waits when another process holds it, and says why,
    /// because the usual cause is a server that outlived its mountpoint and the
    /// user needs to find it rather than block on it.
    ///
    /// `revision` must be [`TreeSnapshot::tree_key`] of the tree this layer
    /// will be stacked on. It scopes the whiteout log: a layer whose log names
    /// a different revision keeps its files and starts with no whiteouts.
    pub fn open(root: PathBuf, revision: &str) -> Result<Self> {
        fs::create_dir_all(&root).map_err(|err| SnapshotError::Io {
            path: root.display().to_string(),
            source: err,
        })?;
        let lock_path = root.join(LOCK_NAME);
        let lock = FileLock::try_lock(lock_path.clone())
            .map_err(|err| SnapshotError::Io {
                path: lock_path.display().to_string(),
                source: err.err,
            })?
            .ok_or_else(|| SnapshotError::OverlayBusy {
                path: root.display().to_string(),
            })?;
        let capacity = NonZeroUsize::new(OPEN_HANDLE_LIMIT).expect("nonzero constant");
        // After the lock, so that two servers cannot both decide the log on
        // disk belongs to somebody else and truncate it.
        let whiteouts = Whiteouts::load(&root, revision)?;
        Ok(Self {
            root,
            _lock: lock,
            state: Mutex::new(UpperState::default()),
            handles: Mutex::new(CLruCache::new(capacity)),
            whiteouts: Mutex::new(whiteouts),
            revision: revision.to_owned(),
        })
    }

    /// The directory holding the upper layer.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Host path backing a repo path.
    fn host_path(&self, path: &RepoPath) -> PathBuf {
        if path.is_root() {
            return self.root.clone();
        }
        // A repo path is `/`-separated by definition and every component has
        // already been checked as a usable filesystem name on the way in.
        self.root.join(path.as_internal_file_string())
    }
}

/// A tree served to a kernel filesystem interface, with or without a writable
/// layer.
///
/// Both adapters talk to this rather than to [`TreeSnapshot`] directly, so that
/// resolution order lives in one place and neither FUSE nor NFS has an opinion
/// about it. With no overlay every read is a straight delegation and every
/// write is `EROFS`, which is exactly the behavior before this module existed.
pub struct OverlayTree {
    lower: Arc<TreeSnapshot>,
    upper: Option<Overlay>,
    stats: Arc<Stats>,
}

impl OverlayTree {
    /// A read-only tree. Every write returns `EROFS`.
    pub fn read_only(lower: Arc<TreeSnapshot>) -> Self {
        Self {
            lower,
            upper: None,
            stats: Arc::new(Stats::new()),
        }
    }

    /// A tree whose writes land in `overlay`.
    pub fn writable(lower: Arc<TreeSnapshot>, overlay: Overlay) -> Self {
        Self {
            lower,
            upper: Some(overlay),
            stats: Arc::new(Stats::new()),
        }
    }

    /// Counters for the operations this layer does on its own account, which is
    /// copy-up and discarded sidecars. Transport procedures are counted by the
    /// adapter.
    pub fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }

    /// Whether writes are accepted at all.
    pub fn is_writable(&self) -> bool {
        self.upper.is_some()
    }

    /// The upper directory, if there is one.
    pub fn upper_root(&self) -> Option<&Path> {
        self.upper.as_ref().map(Overlay::root)
    }

    /// Timestamp reported for anything the revision owns.
    pub fn mtime(&self) -> SystemTime {
        self.lower.mtime()
    }

    /// Mode bits to report for an entry of this kind on this mount.
    pub fn mode_bits(&self, kind: EntryKind) -> u32 {
        match (kind, self.is_writable()) {
            (EntryKind::Directory, true) => MODE_DIR_RW,
            (EntryKind::Directory, false) => MODE_DIR_RO,
            (EntryKind::File { executable: true }, true) => MODE_EXEC_RW,
            (EntryKind::File { executable: true }, false) => MODE_EXEC_RO,
            (EntryKind::File { executable: false }, true) => MODE_FILE_RW,
            (EntryKind::File { executable: false }, false) => MODE_FILE_RO,
            (EntryKind::Symlink, _) => MODE_SYMLINK,
        }
    }

    fn overlay(&self) -> Result<&Overlay> {
        self.upper.as_ref().ok_or(SnapshotError::ReadOnly)
    }
}

// Resolution. Nothing in this block mutates the upper layer.
impl OverlayTree {
    /// Repo path of an inode, whichever layer it came from.
    pub fn path(&self, inode: u64) -> Result<RepoPathBuf> {
        if inode < UPPER_INODE_BASE {
            return self.lower.path(inode);
        }
        let overlay = self.overlay()?;
        let index =
            usize::try_from(inode - UPPER_INODE_BASE).map_err(|_| SnapshotError::NotFound)?;
        let state = overlay.state.lock().expect("lock is poisoned");
        state
            .paths
            .get(index)
            .cloned()
            .ok_or(SnapshotError::NotFound)
    }

    /// Which layer answers for an inode, and where.
    ///
    /// A lower inode whose path has been copied up resolves to `Upper`, which
    /// is what makes a write visible to the next read.
    fn resolve(&self, inode: u64) -> Result<(RepoPathBuf, Layer)> {
        let path = self.path(inode)?;
        let Some(overlay) = self.upper.as_ref() else {
            return Ok((path, Layer::Lower));
        };
        if inode >= UPPER_INODE_BASE {
            if let Some(size) = overlay
                .state
                .lock()
                .expect("lock is poisoned")
                .discarded
                .get(&path)
            {
                return Ok((path, Layer::Discarded(*size)));
            }
            let host = overlay.host_path(&path);
            return Ok((path, Layer::Upper(host)));
        }
        if self.is_shadowed(overlay, &path)? {
            let host = overlay.host_path(&path);
            return Ok((path, Layer::Upper(host)));
        }
        Ok((path, Layer::Lower))
    }

    /// Whether a path the revision contains also exists in the upper layer.
    ///
    /// Probed once with an `lstat` and then remembered. Caching is sound
    /// because the exclusive lock makes this process the only writer and every
    /// mutation below updates the cache in the same breath. Without the cache
    /// this would be one extra syscall on every read of every tracked file.
    fn is_shadowed(&self, overlay: &Overlay, path: &RepoPath) -> Result<bool> {
        if let Some(&hit) = overlay
            .state
            .lock()
            .expect("lock is poisoned")
            .shadowed
            .get(path)
        {
            return Ok(hit);
        }
        // Outside the lock. Holding a mutex across a syscall serializes every
        // other path's lookups behind this one file's `lstat`, which is a
        // scaling bug rather than a correctness one and so does not show up
        // until a build makes thirty thousand of them at once. Racing here is
        // harmless: two threads may both probe, and they get the same answer.
        let exists = fs::symlink_metadata(overlay.host_path(path)).is_ok();
        overlay
            .state
            .lock()
            .expect("lock is poisoned")
            .shadowed
            .insert(path.to_owned(), exists);
        Ok(exists)
    }

    /// Records that `path` now has something in the upper layer.
    fn mark_shadowed(&self, overlay: &Overlay, path: &RepoPath, shadowed: bool) {
        overlay
            .state
            .lock()
            .expect("lock is poisoned")
            .shadowed
            .insert(path.to_owned(), shadowed);
    }

    /// Inode for an upper-only path, allocating one on first sight.
    fn intern_upper(&self, overlay: &Overlay, path: RepoPathBuf) -> u64 {
        let mut state = overlay.state.lock().expect("lock is poisoned");
        if let Some(&inode) = state.by_path.get(&path) {
            return inode;
        }
        let inode = UPPER_INODE_BASE + u64::try_from(state.paths.len()).expect("count fits u64");
        state.by_path.insert(path.clone(), inode);
        state.paths.push(path);
        inode
    }

    /// Inode for a path, preferring the number the revision already gave it.
    fn inode_for(&self, overlay: &Overlay, path: &RepoPath) -> u64 {
        match self.lower.inode_of(path) {
            Some(inode) => {
                // The guard the constant's comment promises. If the two
                // allocators ever meet, every answer past that point is for the
                // wrong file, and a panic is better than serving one.
                assert!(
                    inode < UPPER_INODE_BASE,
                    "lower inode {inode} entered the upper range; the two allocators have met"
                );
                inode
            }
            None => self.intern_upper(overlay, path.to_owned()),
        }
    }

    /// Attributes of one inode.
    pub async fn getattr(&self, inode: u64) -> Result<Attributes> {
        let (_, layer) = self.resolve(inode)?;
        match layer {
            Layer::Lower => self.lower.getattr(inode).await,
            // Reported as a plain file of the size the client last wrote, so
            // the client's own accounting stays consistent with a file that
            // does not exist.
            Layer::Discarded(size) => Ok(Attributes {
                inode,
                kind: EntryKind::File { executable: false },
                size,
                conflicted: false,
                mtime: SystemTime::now(),
            }),
            Layer::Upper(host) => {
                let cached = match self.cached_attributes(inode) {
                    Some(cached) => cached,
                    None => {
                        let metadata = lstat(&host)?;
                        let cached = CachedAttributes {
                            kind: kind_of(&metadata, &host)?,
                            // A symlink's size is the length of its target,
                            // which is what `len()` reports for an `lstat`.
                            size: metadata.len(),
                            mtime: metadata.modified().unwrap_or_else(|_| self.mtime()),
                        };
                        self.remember(inode, cached);
                        cached
                    }
                };
                Ok(Attributes {
                    inode,
                    kind: cached.kind,
                    size: cached.size,
                    conflicted: false,
                    mtime: cached.mtime,
                })
            }
        }
    }

    /// Attributes of an inode, but only when they cost no file read.
    ///
    /// A file in the writable layer is a real file, so its size is one `lstat`.
    /// A file still in the revision may have no known size until its content is
    /// read, because the backend hands out a stream rather than a length, and
    /// paying that to fill in a directory listing is what the read-only mount
    /// deliberately refused to do. `None` means "ask separately if you care".
    pub async fn cheap_getattr(&self, inode: u64) -> Option<Attributes> {
        let (_, layer) = self.resolve(inode).ok()?;
        match layer {
            Layer::Upper(_) | Layer::Discarded(_) => self.getattr(inode).await.ok(),
            Layer::Lower => None,
        }
    }

    /// Resolves one name inside a directory.
    ///
    /// Upper first, so a copied-up file wins over the revision's copy of
    /// itself. No upper entry can hide something a caller still wanted, because
    /// nothing reaches the upper layer except by being written through this
    /// mount.
    pub async fn lookup(&self, parent: u64, name: &str) -> Result<TreeEntry> {
        let Some(overlay) = self.upper.as_ref() else {
            return self.lower.lookup(parent, name).await;
        };
        let (parent_path, _) = self.resolve(parent)?;
        let Ok(component) = RepoPathComponent::new(name) else {
            // A name that cannot be a repo path component, `.` and `..` being
            // the obvious ones, names nothing in either layer.
            return Err(SnapshotError::NotFound);
        };
        let child = parent_path.join(component);
        if is_scratch_name(name) && parent_path.is_root() {
            return Err(SnapshotError::NotFound);
        }
        if is_sidecar(name) {
            // Only a sidecar this mount has been asked to create resolves. A
            // lookup for one that was never created is a miss, which is what
            // lets the client decide to create it.
            let known = overlay
                .state
                .lock()
                .expect("lock is poisoned")
                .discarded
                .contains_key(&child);
            if !known {
                return Err(SnapshotError::NotFound);
            }
            return Ok(TreeEntry {
                inode: self.inode_for(overlay, &child),
                name: name.to_owned(),
                kind: EntryKind::File { executable: false },
                conflicted: false,
            });
        }
        let host = overlay.host_path(&child);
        if let Ok(metadata) = fs::symlink_metadata(&host) {
            return Ok(TreeEntry {
                inode: self.inode_for(overlay, &child),
                name: name.to_owned(),
                kind: kind_of(&metadata, &host)?,
                conflicted: false,
            });
        }
        // One of the two points a whiteout is subtracted. Asked only after the
        // upper layer has missed, so a name that was deleted and then created
        // again resolves to the new file whatever the log says.
        if self.is_hidden(overlay, &child) {
            return Err(SnapshotError::NotFound);
        }
        // Not in the upper layer, so either the revision has it or nobody does.
        // A lookup under an upper-only directory has no lower parent to ask.
        if parent >= UPPER_INODE_BASE {
            return Err(SnapshotError::NotFound);
        }
        self.lower.lookup(parent, name).await
    }

    /// Lists a directory: the union of both layers minus the whiteouts,
    /// ordered by name.
    ///
    /// Union rather than upper-shadows-lower, because an upper directory can
    /// only add names: where a name is in both, the upper one is the copied-up
    /// version of the lower one and describes the same file. Only a whiteout
    /// takes a name away, and it is subtracted from the lower side before the
    /// union so that an upper entry always wins over a stale one.
    pub async fn readdir(&self, inode: u64) -> Result<Vec<TreeEntry>> {
        let Some(overlay) = self.upper.as_ref() else {
            return self.lower.readdir(inode).await;
        };
        let (path, _) = self.resolve(inode)?;
        let mut merged = if inode < UPPER_INODE_BASE {
            self.lower.readdir(inode).await?
        } else {
            Vec::new()
        };
        self.subtract_whiteouts(overlay, &path, &mut merged);
        let mut position: HashMap<String, usize> = merged
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.name.clone(), index))
            .collect();
        for (name, host) in self.upper_entries(overlay, &path)? {
            let Ok(component) = RepoPathComponent::new(&name) else {
                // Nothing could look this name up even if it were listed, so
                // listing it would produce an entry that does not resolve.
                tracing::warn!(name, "skipping upper entry with no repo path name");
                continue;
            };
            let child = path.join(component);
            let metadata = lstat(&host)?;
            let entry = TreeEntry {
                inode: self.inode_for(overlay, &child),
                name: name.clone(),
                kind: kind_of(&metadata, &host)?,
                conflicted: false,
            };
            match position.get(&name) {
                Some(&index) => merged[index] = entry,
                None => {
                    position.insert(name, merged.len());
                    merged.push(entry);
                }
            }
        }
        // Each layer is individually ordered but their union is not, and NFS
        // pagination resumes by position, so the order has to be a function of
        // the contents rather than of which layer answered first.
        merged.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(merged)
    }

    /// Names directly inside a repo path in the upper layer.
    ///
    /// A missing directory is not an error: most paths have no upper
    /// counterpart, and that is the normal case rather than a failure.
    fn upper_entries(&self, overlay: &Overlay, path: &RepoPath) -> Result<Vec<(String, PathBuf)>> {
        let host = overlay.host_path(path);
        let reader = match fs::read_dir(&host) {
            Ok(reader) => reader,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(io_error(&host, err)),
        };
        let mut entries = Vec::new();
        for entry in reader {
            let entry = entry.map_err(|err| io_error(&host, err))?;
            let raw = entry.file_name();
            let Some(name) = raw.to_str() else {
                tracing::warn!(?raw, "skipping upper entry whose name is not UTF-8");
                continue;
            };
            // Only at the root, which is the only place they exist; a
            // revision is free to have `src/.jj-overlay-lock`.
            if path.is_root() && is_scratch_name(name) {
                continue;
            }
            // Belt and braces. Nothing should write one of these to disk any
            // more, but a scratch layer created before the accommodation
            // existed still has thousands of them and should not start showing
            // them now.
            if is_sidecar(name) {
                continue;
            }
            entries.push((name.to_owned(), entry.path()));
        }
        Ok(entries)
    }

    /// Reads up to `len` bytes at `offset`, and whether that reached the end.
    pub async fn read(&self, inode: u64, offset: u64, len: u32) -> Result<(Vec<u8>, bool)> {
        let (_, layer) = self.resolve(inode)?;
        match layer {
            Layer::Lower => self.lower.read(inode, offset, len).await,
            // Nothing was stored, so there is nothing to give back. macOS reads
            // a sidecar to recover an xattr; an empty read means it finds none,
            // which is the intended outcome.
            Layer::Discarded(_) => Ok((Vec::new(), true)),
            Layer::Upper(host) => {
                let file = fs::File::open(&host).map_err(|err| io_error(&host, err))?;
                let size = file.metadata().map_err(|err| io_error(&host, err))?.len();
                if offset >= size {
                    return Ok((Vec::new(), true));
                }
                let want = u64::from(len).min(size - offset);
                let mut buffer = vec![0u8; usize::try_from(want).unwrap_or(usize::MAX)];
                sys::read_exact_at(&file, &mut buffer, offset)
                    .map_err(|err| io_error(&host, err))?;
                Ok((buffer, offset + want >= size))
            }
        }
    }

    /// Reads a symlink target.
    pub async fn readlink(&self, inode: u64) -> Result<Vec<u8>> {
        let (_, layer) = self.resolve(inode)?;
        match layer {
            Layer::Lower => self.lower.readlink(inode).await,
            Layer::Discarded(_) => Err(SnapshotError::NotASymlink {
                path: "AppleDouble sidecar".to_owned(),
            }),
            Layer::Upper(host) => {
                let target = fs::read_link(&host).map_err(|err| io_error(&host, err))?;
                Ok(sys::path_into_bytes(target))
            }
        }
    }

    /// The inode of an entry's parent directory. The root is its own parent.
    pub fn parent(&self, inode: u64) -> Result<u64> {
        if inode < UPPER_INODE_BASE {
            return self.lower.parent(inode);
        }
        let overlay = self.overlay()?;
        let path = self.path(inode)?;
        match path.parent() {
            None => Ok(ROOT_INODE),
            Some(parent) => Ok(self.inode_for(overlay, parent)),
        }
    }
}

// Mutation. Every entry point here starts by asking whether the operation would
// take a name out of the revision's namespace, and refuses if it would.
impl OverlayTree {
    /// Creates a regular file, truncating it if the name is already taken.
    ///
    /// Truncating rather than failing matches `open(O_CREAT | O_TRUNC)` and the
    /// NFSv3 UNCHECKED create mode, which is what a client sends for an
    /// ordinary `>` redirect.
    pub async fn create(&self, parent: u64, name: &str, mode: Option<u32>) -> Result<TreeEntry> {
        let overlay = self.overlay()?;
        let (path, host) = self.child(overlay, parent, name)?;
        if is_sidecar(name) {
            return Ok(self.create_discarded(overlay, path, name));
        }
        // Shadowing a tracked directory with a file would make everything
        // underneath it unreachable, which is a deletion wearing a create's
        // clothes.
        self.refuse_shadowing_a_tracked_directory(parent, name, &path)
            .await?;
        self.ensure_upper_parents(overlay, &path)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&host)
            .map_err(|err| io_error(&host, err))?;
        drop(file);
        if let Some(mode) = mode {
            sys::set_mode(&host, mode).map_err(|err| io_error(&host, err))?;
        }
        self.mark_shadowed(overlay, &path, true);
        self.reveal(overlay, &path)?;
        let inode = self.inode_for(overlay, &path);
        // A create truncates, so anything cached about this inode describes the
        // file that was just replaced.
        self.forget(inode);
        let metadata = lstat(&host)?;
        Ok(TreeEntry {
            inode,
            name: name.to_owned(),
            kind: kind_of(&metadata, &host)?,
            conflicted: false,
        })
    }

    /// Creates a directory. Fails with `EEXIST` if the name is taken in either
    /// layer.
    pub async fn mkdir(&self, parent: u64, name: &str) -> Result<TreeEntry> {
        let overlay = self.overlay()?;
        let (path, host) = self.child(overlay, parent, name)?;
        if self.lookup(parent, name).await.is_ok() {
            return Err(SnapshotError::Exists {
                path: display_path(&path),
            });
        }
        self.ensure_upper_parents(overlay, &path)?;
        fs::create_dir(&host).map_err(|err| io_error(&host, err))?;
        self.mark_shadowed(overlay, &path, true);
        self.reveal(overlay, &path)?;
        Ok(TreeEntry {
            inode: self.inode_for(overlay, &path),
            name: name.to_owned(),
            kind: EntryKind::Directory,
            conflicted: false,
        })
    }

    /// Creates a symlink. Fails with `EEXIST` if the name is taken in either
    /// layer, which is what `symlink(2)` does.
    pub async fn symlink(&self, parent: u64, name: &str, target: &[u8]) -> Result<TreeEntry> {
        let overlay = self.overlay()?;
        let (path, host) = self.child(overlay, parent, name)?;
        if self.lookup(parent, name).await.is_ok() {
            return Err(SnapshotError::Exists {
                path: display_path(&path),
            });
        }
        self.ensure_upper_parents(overlay, &path)?;
        let target = sys::path_from_bytes(target.to_vec()).map_err(|err| io_error(&host, err))?;
        sys::symlink(&target, &host).map_err(|err| io_error(&host, err))?;
        self.mark_shadowed(overlay, &path, true);
        self.reveal(overlay, &path)?;
        Ok(TreeEntry {
            inode: self.inode_for(overlay, &path),
            name: name.to_owned(),
            kind: EntryKind::Symlink,
            conflicted: false,
        })
    }

    /// Writes `data` at `offset`, extending the file if needed. Returns the new
    /// size.
    ///
    /// A file the revision owns is copied into the upper layer first, so the
    /// write lands on a copy and the revision is untouched.
    pub async fn write(&self, inode: u64, offset: u64, data: &[u8]) -> Result<u64> {
        if let Some(size) = self.discard_write(inode, offset, data)? {
            return Ok(size);
        }
        let host = self.for_writing(inode).await?;
        let file = self.write_handle(inode, &host)?;
        sys::write_all_at(&file, data, offset).map_err(|err| io_error(&host, err))?;
        // The new size is computed rather than read back. A `metadata` call
        // here would be a third syscall per write to learn something we just
        // decided, and a write never shrinks a file.
        let written = offset.saturating_add(u64::try_from(data.len()).unwrap_or(0));
        self.grew(inode, written, &host)
    }

    /// Sets a file's length, copying it up first if the revision owns it.
    pub async fn truncate(&self, inode: u64, size: u64) -> Result<()> {
        if self.set_discarded_size(inode, size)? {
            return Ok(());
        }
        let host = self.for_writing(inode).await?;
        let file = self.write_handle(inode, &host)?;
        file.set_len(size).map_err(|err| io_error(&host, err))?;
        self.resized(inode, size);
        Ok(())
    }

    /// Sets a file's mode, copying it up first if the revision owns it.
    ///
    /// Only the permission bits are honored. A caller cannot change what kind
    /// of thing a path is, and the file type bits in a `chmod` are ignored by
    /// every filesystem.
    pub async fn chmod(&self, inode: u64, mode: u32) -> Result<()> {
        // A mode on a file that does not exist is not worth storing, and
        // refusing would fail a `cp -p` whose copy itself worked.
        if matches!(self.resolve(inode)?.1, Layer::Discarded(_)) {
            return Ok(());
        }
        let host = self.for_writing(inode).await?;
        sys::set_mode(&host, mode).map_err(|err| io_error(&host, err))?;
        // The executable bit is part of what a listing reports, so a chmod
        // changes an entry's kind as far as this filesystem is concerned.
        self.forget(inode);
        Ok(())
    }

    /// Removes a file, a symlink, or an empty directory.
    ///
    /// A tracked file or symlink is hidden with a whiteout, whether or not it
    /// had been copied up. A tracked directory is refused: hiding one hides a
    /// subtree, which is the case this module does not implement.
    pub async fn remove(&self, parent: u64, name: &str) -> Result<()> {
        let overlay = self.overlay()?;
        let (path, host) = self.child(overlay, parent, name)?;
        if is_sidecar(name) {
            let forgotten = overlay
                .state
                .lock()
                .expect("lock is poisoned")
                .discarded
                .remove(&path);
            return if forgotten.is_some() {
                Ok(())
            } else {
                Err(SnapshotError::NotFound)
            };
        }
        let tracked = self.lower_kind(parent, name).await?;
        if tracked == Some(EntryKind::Directory) {
            return Err(SnapshotError::Tracked {
                operation: "remove the directory",
                path: display_path(&path),
            });
        }
        let shadowed = self.is_shadowed(overlay, &path)?;
        // A tracked name already hidden is gone as far as any caller can see,
        // so a second remove is `ENOENT` rather than a second whiteout.
        if tracked.is_some() && !shadowed && self.is_hidden(overlay, &path) {
            return Err(SnapshotError::NotFound);
        }
        // A tracked name that was never written to has nothing on disk to
        // unlink; the whiteout is the whole operation.
        if shadowed || tracked.is_none() {
            let metadata = lstat(&host)?;
            let result = if metadata.is_dir() {
                fs::remove_dir(&host)
            } else {
                fs::remove_file(&host)
            };
            result.map_err(|err| match err.kind() {
                // Kept distinct from a generic I/O error because it is the
                // difference between a shell reporting something a user can act
                // on and not. Matched on `ErrorKind` rather than on
                // `libc::ENOTEMPTY`: Windows raises `ERROR_DIR_NOT_EMPTY`, which
                // is 145 and not that errno, so the errno comparison silently
                // fell through to `Io` there.
                io::ErrorKind::DirectoryNotEmpty => SnapshotError::NotEmpty {
                    path: display_path(&path),
                },
                _ => io_error(&host, err),
            })?;
        }
        if tracked.is_some() {
            self.hide(overlay, &path)?;
        }
        self.mark_shadowed(overlay, &path, false);
        // The path can be created again, and `intern_upper` hands back the same
        // inode when it is, so leaving this cached would describe a file that
        // no longer exists to a caller who recreated it.
        let inode = self.inode_for(overlay, &path);
        self.forget(inode);
        Ok(())
    }

    /// Renames a name to another name, in either direction across the layers.
    ///
    /// A tracked file or symlink source is copied up and then hidden, which is
    /// what makes `mv old new` and the replace-a-lockfile idiom work. A tracked
    /// directory source is refused, for the same reason removing one is.
    ///
    /// The destination may be a tracked file or symlink, which is how a
    /// write-to-temp-then-rename tool updates a tracked file. A tracked
    /// directory destination is refused for the same reason a create over one
    /// is: it would hide everything underneath.
    pub async fn rename(
        &self,
        from_parent: u64,
        from_name: &str,
        to_parent: u64,
        to_name: &str,
    ) -> Result<()> {
        let overlay = self.overlay()?;
        let (from_path, from_host) = self.child(overlay, from_parent, from_name)?;
        let (to_path, to_host) = self.child(overlay, to_parent, to_name)?;
        // A sidecar rename accompanies a rename of the real file, so it has to
        // be accepted rather than refused, but there is nothing on disk to move.
        if is_sidecar(from_name) || is_sidecar(to_name) {
            let mut state = overlay.state.lock().expect("lock is poisoned");
            let size = state.discarded.remove(&from_path).unwrap_or(0);
            state.discarded.insert(to_path, size);
            return Ok(());
        }
        let tracked = self.lower_kind(from_parent, from_name).await?;
        if tracked == Some(EntryKind::Directory) {
            return Err(SnapshotError::Tracked {
                operation: "rename away the directory",
                path: display_path(&from_path),
            });
        }
        self.refuse_shadowing_a_tracked_directory(to_parent, to_name, &to_path)
            .await?;
        self.ensure_upper_parents(overlay, &to_path)?;
        if tracked.is_some() && !self.is_shadowed(overlay, &from_path)? {
            if self.is_hidden(overlay, &from_path) {
                return Err(SnapshotError::NotFound);
            }
            // `rename` moves a real file, and a tracked name that has never
            // been written to has no real file yet. Copying up first is what
            // gives the destination the revision's content.
            let from_inode = self.inode_for(overlay, &from_path);
            self.copy_up(overlay, from_inode, &from_path).await?;
        }
        fs::rename(&from_host, &to_host).map_err(|err| io_error(&from_host, err))?;
        if tracked.is_some() {
            self.hide(overlay, &from_path)?;
        }
        self.reveal(overlay, &to_path)?;
        self.mark_shadowed(overlay, &from_path, false);
        self.mark_shadowed(overlay, &to_path, true);
        // Both ends change what they name. The source no longer exists and the
        // destination now holds different content, and a descriptor held for
        // either would be open on the wrong file.
        let from_inode = self.inode_for(overlay, &from_path);
        let to_inode = self.inode_for(overlay, &to_path);
        self.forget(from_inode);
        self.forget(to_inode);
        Ok(())
    }

    /// Cached attributes for an inode in the writable layer.
    fn cached_attributes(&self, inode: u64) -> Option<CachedAttributes> {
        self.upper
            .as_ref()?
            .state
            .lock()
            .expect("lock is poisoned")
            .attributes
            .get(&inode)
            .copied()
    }

    /// Records what we now know about an inode.
    fn remember(&self, inode: u64, attributes: CachedAttributes) {
        let Some(overlay) = self.upper.as_ref() else {
            return;
        };
        overlay
            .state
            .lock()
            .expect("lock is poisoned")
            .attributes
            .insert(inode, attributes);
    }

    /// Drops what we knew, so the next `getattr` asks the filesystem.
    ///
    /// Also drops any held descriptor, because the two are invalidated by the
    /// same events and a stale handle on a replaced file is worse than a stale
    /// size.
    fn forget(&self, inode: u64) {
        let Some(overlay) = self.upper.as_ref() else {
            return;
        };
        overlay
            .state
            .lock()
            .expect("lock is poisoned")
            .attributes
            .remove(&inode);
        overlay
            .handles
            .lock()
            .expect("lock is poisoned")
            .pop(&inode);
    }

    /// Records that a write extended a file to `written` bytes, returning the
    /// size the caller should be told.
    fn grew(&self, inode: u64, written: u64, host: &Path) -> Result<u64> {
        let Some(overlay) = self.upper.as_ref() else {
            return Ok(written);
        };
        let mut state = overlay.state.lock().expect("lock is poisoned");
        match state.attributes.get_mut(&inode) {
            Some(cached) => {
                cached.size = cached.size.max(written);
                cached.mtime = SystemTime::now();
                Ok(cached.size)
            }
            None => {
                // Nothing cached yet, so this write is the first thing we know
                // about the file and the real size has to come from the
                // filesystem. Happens once per file at most.
                drop(state);
                let metadata = lstat(host)?;
                let cached = CachedAttributes {
                    kind: kind_of(&metadata, host)?,
                    size: metadata.len(),
                    mtime: metadata.modified().unwrap_or_else(|_| SystemTime::now()),
                };
                self.remember(inode, cached);
                Ok(cached.size)
            }
        }
    }

    /// Records that a file was set to an exact length.
    fn resized(&self, inode: u64, size: u64) {
        let Some(overlay) = self.upper.as_ref() else {
            return;
        };
        let mut state = overlay.state.lock().expect("lock is poisoned");
        if let Some(cached) = state.attributes.get_mut(&inode) {
            cached.size = size;
            cached.mtime = SystemTime::now();
        }
    }

    /// A writable descriptor for an inode, opened once and then held.
    fn write_handle(&self, inode: u64, host: &Path) -> Result<Arc<fs::File>> {
        let overlay = self.overlay()?;
        if let Some(handle) = overlay
            .handles
            .lock()
            .expect("lock is poisoned")
            .get(&inode)
            .cloned()
        {
            return Ok(handle);
        }
        // Opened outside the cache lock, for the same reason `is_shadowed`
        // probes outside its own: an `open` under a shared lock serializes
        // every other file's writes behind this one.
        let file = Arc::new(
            fs::OpenOptions::new()
                .write(true)
                .open(host)
                .map_err(|err| io_error(host, err))?,
        );
        // A racing thread may have inserted one meanwhile. Either descriptor is
        // correct, so keep whichever is already there and let ours close.
        let mut handles = overlay.handles.lock().expect("lock is poisoned");
        if let Some(existing) = handles.get(&inode).cloned() {
            return Ok(existing);
        }
        handles.put(inode, file.clone());
        Ok(file)
    }

    /// Registers a sidecar the client asked to create, storing nothing.
    fn create_discarded(&self, overlay: &Overlay, path: RepoPathBuf, name: &str) -> TreeEntry {
        let inode = self.intern_upper(overlay, path.clone());
        overlay
            .state
            .lock()
            .expect("lock is poisoned")
            .discarded
            .insert(path, 0);
        self.stats.record(Op::Sidecar, 0, 0);
        TreeEntry {
            inode,
            name: name.to_owned(),
            kind: EntryKind::File { executable: false },
            conflicted: false,
        }
    }

    /// Swallows a write to a sidecar, returning the size the client should be
    /// told. `None` means this inode is a real file and the caller should
    /// carry on.
    fn discard_write(&self, inode: u64, offset: u64, data: &[u8]) -> Result<Option<u64>> {
        let (path, layer) = self.resolve(inode)?;
        let Layer::Discarded(size) = layer else {
            return Ok(None);
        };
        let overlay = self.overlay()?;
        let written = offset.saturating_add(u64::try_from(data.len()).unwrap_or(0));
        let new_size = size.max(written);
        overlay
            .state
            .lock()
            .expect("lock is poisoned")
            .discarded
            .insert(path, new_size);
        self.stats
            .record(Op::Sidecar, 0, u64::try_from(data.len()).unwrap_or(0));
        Ok(Some(new_size))
    }

    /// Sets a sidecar's claimed size. Returns whether this inode was one.
    fn set_discarded_size(&self, inode: u64, size: u64) -> Result<bool> {
        let (path, layer) = self.resolve(inode)?;
        if !matches!(layer, Layer::Discarded(_)) {
            return Ok(false);
        }
        let overlay = self.overlay()?;
        overlay
            .state
            .lock()
            .expect("lock is poisoned")
            .discarded
            .insert(path, size);
        Ok(true)
    }

    /// The host path to write to, copying the revision's content up first if
    /// this inode still resolves to the lower layer.
    async fn for_writing(&self, inode: u64) -> Result<PathBuf> {
        let overlay = self.overlay()?;
        let (path, layer) = self.resolve(inode)?;
        if let Layer::Upper(host) = layer {
            return Ok(host);
        }
        self.copy_up(overlay, inode, &path).await
    }

    /// Copies a path's current content out of the revision and into the upper
    /// layer, so a write can land on it.
    ///
    /// This is the whole cost of supporting writes to tracked files: read the
    /// blob once, write it once, and from then on the path resolves upper. It
    /// goes through the content cache rather than around it, so a file that was
    /// just read is not read again.
    async fn copy_up(&self, overlay: &Overlay, inode: u64, path: &RepoPath) -> Result<PathBuf> {
        let mut timer = Timer::new(&self.stats, Op::CopyUp);
        let attributes = self.lower.getattr(inode).await?;
        timer.bytes(attributes.size);
        let host = overlay.host_path(path);
        self.ensure_upper_parents(overlay, path)?;
        match attributes.kind {
            EntryKind::Directory => {
                // Nothing to copy: a directory in the upper layer is a container
                // for entries, and its lower children stay where they are and
                // keep showing up through the readdir union.
                fs::create_dir_all(&host).map_err(|err| io_error(&host, err))?;
            }
            EntryKind::Symlink => {
                let target = self.lower.readlink(inode).await?;
                let target = sys::path_from_bytes(target).map_err(|err| io_error(&host, err))?;
                sys::symlink(&target, &host).map_err(|err| io_error(&host, err))?;
            }
            EntryKind::File { executable } => {
                let content = self.lower.content_bytes(inode).await?;
                // Written through a temporary and renamed into place, so a
                // crash mid-copy cannot leave a truncated file shadowing an
                // intact one. That failure would look exactly like data loss,
                // since the shadow is what every later read returns.
                let temporary = host.with_extension("jj-copyup");
                fs::write(&temporary, content.as_slice())
                    .map_err(|err| io_error(&temporary, err))?;
                let mode = if executable {
                    MODE_EXEC_RW
                } else {
                    MODE_FILE_RW
                };
                sys::set_mode(&temporary, mode).map_err(|err| io_error(&temporary, err))?;
                fs::rename(&temporary, &host).map_err(|err| io_error(&host, err))?;
            }
        }
        self.mark_shadowed(overlay, path, true);
        // The path resolved to the revision until this instant, so nothing
        // should be cached for it, but copy-up installs the file by renaming
        // over it and a descriptor opened a moment earlier would be on the
        // temporary. Cheap to be certain.
        self.forget(inode);
        Ok(host)
    }

    /// Creates the ancestor directories of `path` in the upper layer.
    ///
    /// These are containers, not entries. A container over a tracked directory
    /// adds nothing to the listing, because the readdir union takes both sides
    /// and an empty upper directory contributes no names.
    fn ensure_upper_parents(&self, overlay: &Overlay, path: &RepoPath) -> Result<()> {
        let Some(parent) = path.parent() else {
            return Ok(());
        };
        let host = overlay.host_path(parent);
        fs::create_dir_all(&host).map_err(|err| io_error(&host, err))?;
        // Every ancestor now has something in the upper layer, and a stale
        // "not shadowed" answer for one of them would send a later lookup to
        // the lower layer for a path only the upper layer has.
        let mut current = Some(parent);
        while let Some(ancestor) = current {
            self.mark_shadowed(overlay, ancestor, true);
            current = ancestor.parent();
        }
        Ok(())
    }

    /// The repo path and host path of a name inside a directory.
    fn child(&self, overlay: &Overlay, parent: u64, name: &str) -> Result<(RepoPathBuf, PathBuf)> {
        let (parent_path, _) = self.resolve(parent)?;
        if is_scratch_name(name) && parent_path.is_root() {
            return Err(SnapshotError::InvalidName {
                name: name.to_owned(),
            });
        }
        let component = RepoPathComponent::new(name).map_err(|_| SnapshotError::InvalidName {
            name: name.to_owned(),
        })?;
        let path = parent_path.join(component);
        let host = overlay.host_path(&path);
        Ok((path, host))
    }

    /// What the revision has at a name inside a directory, if anything.
    ///
    /// Asked of the lower layer directly rather than of the merged view, since
    /// the merged view cannot tell a copied-up file from an upper-only one, and
    /// that distinction decides whether an operation needs a whiteout. It also
    /// deliberately ignores whiteouts: a hidden tracked name is still tracked,
    /// and the caller wants to know that.
    async fn lower_kind(&self, parent: u64, name: &str) -> Result<Option<EntryKind>> {
        if parent >= UPPER_INODE_BASE {
            return Ok(None);
        }
        match self.lower.lookup(parent, name).await {
            Ok(entry) => Ok(Some(entry.kind)),
            Err(SnapshotError::NotFound) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Whether a tracked name has been deleted through this mount.
    fn is_hidden(&self, overlay: &Overlay, path: &RepoPath) -> bool {
        overlay
            .whiteouts
            .lock()
            .expect("lock is poisoned")
            .contains(path)
    }

    /// Hides a tracked name, persisting the fact.
    fn hide(&self, overlay: &Overlay, path: &RepoPath) -> Result<()> {
        let hidden = overlay
            .whiteouts
            .lock()
            .expect("lock is poisoned")
            .hide(path, &overlay.revision)?;
        if hidden {
            self.stats.record(Op::Whiteout, 0, 0);
        }
        Ok(())
    }

    /// Un-hides a name, because something has created it again.
    fn reveal(&self, overlay: &Overlay, path: &RepoPath) -> Result<()> {
        overlay
            .whiteouts
            .lock()
            .expect("lock is poisoned")
            .reveal(path, &overlay.revision)?;
        Ok(())
    }

    /// Drops the entries a whiteout hides from a listing of the lower layer.
    ///
    /// One lock for the whole directory rather than one per entry, because a
    /// listing of a large directory is where the cost of this would show up.
    fn subtract_whiteouts(&self, overlay: &Overlay, path: &RepoPath, entries: &mut Vec<TreeEntry>) {
        let whiteouts = overlay.whiteouts.lock().expect("lock is poisoned");
        if whiteouts.hidden.is_empty() {
            return;
        }
        entries.retain(|entry| match RepoPathComponent::new(&entry.name) {
            Ok(component) => !whiteouts.contains(&path.join(component)),
            // Not a name a whiteout could ever have been recorded under.
            Err(_) => true,
        });
    }

    /// Refuses an operation that would put a non-directory over a directory the
    /// revision owns.
    ///
    /// Allowed for a tracked file or symlink, which is the copy-up case.
    /// Refused for a tracked directory, because the readdir union takes the
    /// upper entry and everything below the directory would stop resolving,
    /// which is a deletion by another name.
    async fn refuse_shadowing_a_tracked_directory(
        &self,
        parent: u64,
        name: &str,
        path: &RepoPath,
    ) -> Result<()> {
        if parent >= UPPER_INODE_BASE {
            return Ok(());
        }
        match self.lower.lookup(parent, name).await {
            Ok(entry) if entry.kind == EntryKind::Directory => Err(SnapshotError::Tracked {
                operation: "replace the directory",
                path: display_path(path),
            }),
            Ok(_) => Ok(()),
            Err(SnapshotError::NotFound) => Ok(()),
            Err(err) => Err(err),
        }
    }
}

/// Reads `metadata` back as one of the three kinds a tree can hold.
///
/// A socket, fifo or device node in the upper layer has no representation in a
/// jj tree, so it is refused rather than guessed at.
fn kind_of(metadata: &fs::Metadata, host: &Path) -> Result<EntryKind> {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        Ok(EntryKind::Directory)
    } else if file_type.is_symlink() {
        Ok(EntryKind::Symlink)
    } else if file_type.is_file() {
        Ok(EntryKind::File {
            executable: sys::is_executable(metadata),
        })
    } else {
        Err(SnapshotError::Unsupported {
            path: host.display().to_string(),
        })
    }
}

fn lstat(host: &Path) -> Result<fs::Metadata> {
    fs::symlink_metadata(host).map_err(|err| io_error(host, err))
}

/// Maps a host filesystem error, keeping "not found" distinct.
///
/// A missing upper file is the ordinary case of a path that lives only in the
/// revision, not an I/O failure, and reporting it as `EIO` would turn every
/// such lookup into an error the caller cannot recover from.
fn io_error(host: &Path, err: io::Error) -> SnapshotError {
    if err.kind() == io::ErrorKind::NotFound {
        return SnapshotError::NotFound;
    }
    SnapshotError::Io {
        path: host.display().to_string(),
        source: err,
    }
}

fn display_path(path: &RepoPath) -> String {
    if path.is_root() {
        "<root>".to_owned()
    } else {
        path.as_internal_file_string().to_owned()
    }
}
