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

//! The transport-agnostic core: a read-only view of one commit's tree.
//!
//! Everything here is expressed in terms a kernel filesystem interface asks
//! for (inode numbers, sizes, directory listings) but nothing here knows which
//! interface is asking. Both the FUSE and the NFS adapter are thin translation
//! layers over [`TreeSnapshot`].

use std::collections::HashMap;
use std::collections::hash_map::RandomState;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::SystemTime;

use clru::CLruCache;
use clru::WeightScale;
use jj_lib::backend::BackendError;
use jj_lib::backend::Timestamp;
use jj_lib::backend::TreeValue;
use jj_lib::conflict_labels::ConflictLabels;
use jj_lib::conflicts::ConflictMarkerStyle;
use jj_lib::conflicts::ConflictMaterializeOptions;
use jj_lib::conflicts::MaterializedTreeValue;
use jj_lib::conflicts::materialize_merge_result_to_bytes;
use jj_lib::conflicts::materialize_tree_value;
use jj_lib::conflicts::resolve_file_executable;
use jj_lib::merge::Merge;
use jj_lib::merge::MergedTreeVal;
use jj_lib::merge::MergedTreeValue;
use jj_lib::merged_tree::MergedTree;
use jj_lib::merged_tree::all_merged_tree_entries;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo_path::RepoPath;
use jj_lib::repo_path::RepoPathBuf;
use jj_lib::store::Store;
use jj_lib::tree::Tree;
use jj_lib::tree_merge::MergeOptions;

/// Inode number of the mount root.
///
/// FUSE fixes this at 1, so the core uses the same value for every transport
/// rather than making each adapter remap it.
pub const ROOT_INODE: u64 = 1;

/// Inode 0 is reserved by both FUSE and NFSv3, so allocation starts above the
/// root.
const FIRST_CHILD_INODE: u64 = 2;

/// Reported size of a directory.
///
/// Every real filesystem reports something here and no caller can do anything
/// useful with the number, but computing a true value would mean reading the
/// directory on every getattr, which NFS calls constantly. One block is the
/// conventional answer for a small directory.
const DIRECTORY_SIZE: u64 = 4096;

/// Default byte budget for cached file contents.
///
/// A cache is not an optimization here, it is required for correctness of
/// `getattr`: the jj backend hands out a content stream with no length, so the
/// only way to learn a file's size is to read it, and a filesystem must report
/// a size before anyone reads the file. See `content()`.
pub const DEFAULT_CONTENT_CACHE_BYTES: usize = 256 << 20;

/// What kind of thing lives at a path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryKind {
    /// A directory.
    Directory,
    /// A regular file.
    File {
        /// Whether the executable bit is set.
        executable: bool,
    },
    /// A symbolic link.
    Symlink,
}

/// One entry as returned by a directory listing.
#[derive(Clone, Debug)]
pub struct TreeEntry {
    /// Inode number, stable for the life of the [`TreeSnapshot`].
    pub inode: u64,
    /// Entry name, already validated as usable as a filesystem name.
    pub name: String,
    /// What kind of thing this is.
    pub kind: EntryKind,
    /// Whether the tree has a conflict at this path. See the crate docs for
    /// what a conflicted path looks like through the mount.
    pub conflicted: bool,
}

/// Everything a `getattr` needs to answer.
#[derive(Clone, Copy, Debug)]
pub struct Attributes {
    /// Inode number.
    pub inode: u64,
    /// What kind of thing this is.
    pub kind: EntryKind,
    /// Size in bytes. For a symlink, the length of the target.
    pub size: u64,
    /// Whether the tree has a conflict at this path.
    pub conflicted: bool,
    /// Modification time.
    ///
    /// Per entry rather than per mount because a writable overlay serves real
    /// files whose timestamps move, and every incremental build system decides
    /// what to rebuild by comparing them. Reporting the commit's timestamp for
    /// a file `bun` wrote thirty seconds ago makes `make` and friends skip work
    /// they have to do.
    pub mtime: SystemTime,
}

/// Why a filesystem operation could not be answered.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// No entry at that name or inode.
    #[error("No such file or directory")]
    NotFound,
    /// A directory operation was asked of something that is not a directory.
    #[error("Not a directory: {path}")]
    NotADirectory {
        /// The path that is not a directory.
        path: String,
    },
    /// `readlink` was asked of something that is not a symlink.
    #[error("Not a symbolic link: {path}")]
    NotASymlink {
        /// The path that is not a symlink.
        path: String,
    },
    /// A read was asked of a directory.
    #[error("Is a directory: {path}")]
    IsADirectory {
        /// The path that is a directory.
        path: String,
    },
    /// The name in the tree cannot be represented as a filesystem name, for
    /// example `.` or `..`.
    #[error("Invalid file name in tree: {name}")]
    InvalidName {
        /// The offending name, as stored in the tree.
        name: String,
    },
    /// The backend refused to hand over the content.
    #[error("Access denied reading {path}: {source}")]
    AccessDenied {
        /// The path that could not be read.
        path: String,
        /// The underlying reason.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The backend failed.
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// Rendering a conflict into bytes failed.
    #[error("Failed to materialize conflict at {path}: {source}")]
    Materialize {
        /// The conflicted path.
        path: String,
        /// The underlying reason.
        source: std::io::Error,
    },
    /// A write was attempted against a mount that has no writable layer.
    #[error("Read-only file system")]
    ReadOnly,
    /// A write was attempted against something the revision contains, in a way
    /// that would remove it from the revision's namespace.
    ///
    /// Carries the operation as well as the path. Without it every refusal
    /// reads the same, and a caller looking at `EROFS` from a tool that made
    /// several calls cannot tell which one was refused. That cost an hour the
    /// first time: `bun install` reports only "EROFS", and narrowing it to a
    /// specific unlink took a round trip through a rebuilt server.
    #[error("cannot {operation} {path}, which is in the revision")]
    Tracked {
        /// What was attempted, as a verb phrase.
        operation: &'static str,
        /// The path the revision contains.
        path: String,
    },
    /// The name is already taken.
    #[error("File exists: {path}")]
    Exists {
        /// The path that already exists.
        path: String,
    },
    /// A directory that still has entries cannot be removed.
    #[error("Directory not empty: {path}")]
    NotEmpty {
        /// The directory that still has entries.
        path: String,
    },
    /// The upper layer holds something a jj tree cannot represent, such as a
    /// socket or a device node.
    #[error("Unsupported file type: {path}")]
    Unsupported {
        /// The offending path.
        path: String,
    },
    /// Another process holds the upper layer.
    #[error(
        "Overlay {path} is in use by another process. A `jj fs mount` can outlive its mountpoint; \
         look for one still running."
    )]
    OverlayBusy {
        /// The upper directory.
        path: String,
    },
    /// The host filesystem holding the upper layer failed.
    #[error("I/O error on {path}: {source}")]
    Io {
        /// The host path.
        path: String,
        /// The underlying reason.
        source: std::io::Error,
    },
}

impl SnapshotError {
    /// The POSIX errno a kernel client should be told.
    ///
    /// Backend and materialization failures become `EIO` because there is no
    /// errno for "the object store is unhappy", and a filesystem client has to
    /// be given something it can act on.
    ///
    /// **Meaningful on unix only.** The `Io` arm passes the host's own error
    /// number straight through, and on Windows that is a Win32 code rather than
    /// an errno: `ERROR_DIR_NOT_EMPTY` would be reported as 145, which is not
    /// `ENOTEMPTY` or anything else. Sound today because the only caller is the
    /// FUSE adapter, which is Linux-only, and the NFS adapter maps the error
    /// enum to an `nfsstat3` without going through here. A Windows client would
    /// need that mapping rather than this.
    pub fn errno(&self) -> i32 {
        match self {
            Self::NotFound => libc::ENOENT,
            Self::NotADirectory { .. } => libc::ENOTDIR,
            Self::NotASymlink { .. } => libc::EINVAL,
            Self::IsADirectory { .. } => libc::EISDIR,
            Self::InvalidName { .. } => libc::EINVAL,
            Self::AccessDenied { .. } => libc::EACCES,
            // EROFS rather than EACCES or EPERM. A caller that sees EROFS knows
            // to stop trying, where EACCES reads as "try as another user" and
            // sends people looking for a permissions problem that is not there.
            Self::ReadOnly | Self::Tracked { .. } => libc::EROFS,
            Self::Exists { .. } => libc::EEXIST,
            Self::NotEmpty { .. } => libc::ENOTEMPTY,
            Self::Unsupported { .. } => libc::EOPNOTSUPP,
            Self::OverlayBusy { .. } => libc::EBUSY,
            Self::Io { source, .. } => source.raw_os_error().unwrap_or(libc::EIO),
            Self::Backend(_) | Self::Materialize { .. } => libc::EIO,
        }
    }
}

type Result<T> = std::result::Result<T, SnapshotError>;

/// What one inode number refers to.
#[derive(Clone, Debug)]
struct Inode {
    path: RepoPathBuf,
    /// The tree value, kept here so that reading a file does not have to walk
    /// the tree from the root again. It is a handful of ids, not content.
    value: MergedTreeValue,
    kind: EntryKind,
    conflicted: bool,
}

/// Path to inode mapping, plus the reverse.
///
/// Inodes are handed out by a counter and never reused. The alternative a
/// reader would consider is hashing the path or the content id down to 64
/// bits, which would be stable across mounts and need no table at all, but two
/// colliding paths would then silently alias to one file, and both FUSE and
/// NFS treat inode identity as authoritative. A counter cannot collide. The
/// cost is that inodes are not stable across mounts, which matters only for
/// NFS file handle reuse, and nfs3_server already stamps a per-startup
/// generation number into the handle so a restart yields ESTALE rather than
/// wrong data.
#[derive(Debug)]
struct InodeTable {
    by_path: HashMap<RepoPathBuf, u64>,
    /// Indexed by `inode - ROOT_INODE`.
    entries: Vec<Inode>,
}

impl InodeTable {
    fn new(root: Inode) -> Self {
        let mut by_path = HashMap::new();
        by_path.insert(root.path.clone(), ROOT_INODE);
        Self {
            by_path,
            entries: vec![root],
        }
    }

    fn get(&self, inode: u64) -> Option<&Inode> {
        let index = inode.checked_sub(ROOT_INODE)?;
        self.entries.get(usize::try_from(index).ok()?)
    }

    /// Returns the inode for `path`, allocating one on first sight.
    fn intern(
        &mut self,
        path: RepoPathBuf,
        value: MergedTreeValue,
        kind: EntryKind,
        conflicted: bool,
    ) -> u64 {
        if let Some(&inode) = self.by_path.get(&path) {
            return inode;
        }
        let inode = ROOT_INODE + u64::try_from(self.entries.len()).expect("inode count fits u64");
        debug_assert!(inode >= FIRST_CHILD_INODE);
        self.by_path.insert(path.clone(), inode);
        self.entries.push(Inode {
            path,
            value,
            kind,
            conflicted,
        });
        inode
    }
}

/// A directory listing, kept in both orders a filesystem asks for.
#[derive(Debug)]
struct Directory {
    /// Sorted by name, because that is how a jj tree stores entries. The order
    /// has to be identical across calls: NFSv3 readdir pagination resumes from
    /// a cookie handed out by an earlier call, so a listing that reshuffles
    /// silently skips or repeats entries.
    entries: Vec<TreeEntry>,
    by_name: HashMap<String, usize>,
}

/// Charge a cached file its own length, so the budget is in bytes rather than
/// in entries. An entry count is the wrong unit when one file can be a gigabyte
/// and the next can be empty.
struct ContentWeight;

impl WeightScale<u64, Arc<Vec<u8>>> for ContentWeight {
    fn weight(&self, _key: &u64, value: &Arc<Vec<u8>>) -> usize {
        // clru requires a strictly positive weight to make progress, and an
        // empty file is a real cache entry worth remembering.
        value.len().max(1)
    }
}

/// A read-only view of one commit's tree, addressed by inode.
pub struct TreeSnapshot {
    store: Arc<Store>,
    root_trees: Merge<Tree>,
    labels: ConflictLabels,
    materialize: ConflictMaterializeOptions,
    mtime: SystemTime,
    inodes: Mutex<InodeTable>,
    directories: Mutex<HashMap<u64, Arc<Directory>>>,
    contents: Mutex<CLruCache<u64, Arc<Vec<u8>>, RandomState, ContentWeight>>,
}

impl TreeSnapshot {
    /// Reads the root trees of `tree` and prepares to serve it.
    ///
    /// `mtime` is reported for every entry; callers normally pass the commit's
    /// committer timestamp so that the mount looks as old as the commit rather
    /// than as old as the mount.
    pub async fn new(
        tree: &MergedTree,
        materialize: ConflictMaterializeOptions,
        mtime: &Timestamp,
        content_cache_bytes: usize,
    ) -> Result<Self> {
        let store = tree.store().clone();
        // Taken from the tree rather than accepted as a parameter: the labels
        // have to be the ones belonging to this tree, and there is no reason a
        // caller would supply others.
        let labels = tree.labels().clone();
        let root_trees = tree.trees().await?;
        let root_value = tree.tree_ids().map(|id| Some(TreeValue::Tree(id.clone())));
        let conflicted = !root_value.is_resolved();
        let root = Inode {
            path: RepoPathBuf::root(),
            value: root_value,
            kind: EntryKind::Directory,
            conflicted,
        };
        let capacity = NonZeroUsize::new(content_cache_bytes)
            .unwrap_or(NonZeroUsize::new(DEFAULT_CONTENT_CACHE_BYTES).expect("nonzero constant"));
        Ok(Self {
            store,
            root_trees,
            labels,
            materialize,
            mtime: timestamp_to_system_time(mtime),
            inodes: Mutex::new(InodeTable::new(root)),
            directories: Mutex::new(HashMap::new()),
            contents: Mutex::new(CLruCache::with_scale(capacity, ContentWeight)),
        })
    }

    /// Timestamp reported as atime, mtime and ctime for every entry.
    pub fn mtime(&self) -> SystemTime {
        self.mtime
    }

    /// A stable identity for the tree being served.
    ///
    /// Anything persisted about *this* revision's names, rather than about
    /// paths in general, is scoped by this so that it does not carry over to a
    /// different revision. The writable overlay's whiteouts are the only such
    /// thing today; see [`crate::overlay`].
    ///
    /// Every root tree id, so a conflicted tree with several roots is a
    /// different key from any one of them alone. Hex rather than raw bytes
    /// because it goes into a text file a person may have to read.
    pub fn tree_key(&self) -> String {
        self.root_trees
            .iter()
            .map(|tree| tree.id().hex())
            .collect::<Vec<_>>()
            .join("+")
    }

    /// Bytes of file content currently held in the cache.
    ///
    /// Exposed because it is the only way to tell whether an operation read a
    /// file or answered from metadata, which is the difference between a
    /// directory listing that is fast and one that is not.
    pub fn cached_content_bytes(&self) -> usize {
        self.contents.lock().expect("lock is poisoned").weight()
    }

    /// Attributes of one inode.
    pub async fn getattr(&self, inode: u64) -> Result<Attributes> {
        let entry = self.inode(inode)?;
        let size = match entry.kind {
            EntryKind::Directory => DIRECTORY_SIZE,
            _ => self.size(inode, &entry).await?,
        };
        Ok(Attributes {
            inode,
            kind: entry.kind,
            size,
            conflicted: entry.conflicted,
            mtime: self.mtime,
        })
    }

    /// Resolves one name inside a directory.
    pub async fn lookup(&self, parent: u64, name: &str) -> Result<TreeEntry> {
        let directory = self.directory(parent).await?;
        let index = *directory.by_name.get(name).ok_or(SnapshotError::NotFound)?;
        Ok(directory.entries[index].clone())
    }

    /// Lists a directory. The order is stable across calls.
    pub async fn readdir(&self, inode: u64) -> Result<Vec<TreeEntry>> {
        Ok(self.directory(inode).await?.entries.clone())
    }

    /// Reads up to `len` bytes of a file at `offset`.
    ///
    /// Returns the bytes and whether the read reached end of file, which NFSv3
    /// requires the server to report explicitly.
    pub async fn read(&self, inode: u64, offset: u64, len: u32) -> Result<(Vec<u8>, bool)> {
        let entry = self.inode(inode)?;
        if entry.kind == EntryKind::Directory {
            return Err(SnapshotError::IsADirectory {
                path: display_path(&entry.path),
            });
        }
        let content = self.content(inode).await?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(content.len());
        let end = start.saturating_add(len as usize).min(content.len());
        Ok((content[start..end].to_vec(), end == content.len()))
    }

    /// Reads a symlink target.
    pub async fn readlink(&self, inode: u64) -> Result<Vec<u8>> {
        let entry = self.inode(inode)?;
        if entry.kind != EntryKind::Symlink {
            return Err(SnapshotError::NotASymlink {
                path: display_path(&entry.path),
            });
        }
        Ok(self.content(inode).await?.as_ref().clone())
    }

    /// Repository path of an inode, for diagnostics.
    pub fn path(&self, inode: u64) -> Result<RepoPathBuf> {
        Ok(self.inode(inode)?.path)
    }

    /// The inode already assigned to a path, if the path has been listed.
    ///
    /// Only interned paths are known, which is every path a caller can name:
    /// nothing reaches a child without having listed the directory holding it.
    /// The overlay uses this so that copying a file up keeps the inode number
    /// the revision gave it, since an NFS file handle is built from the inode
    /// and changing it mid-write hands the client `ESTALE` on its own file.
    pub fn inode_of(&self, path: &RepoPath) -> Option<u64> {
        self.inodes
            .lock()
            .expect("lock is poisoned")
            .by_path
            .get(path)
            .copied()
    }

    /// Whole content of a file or symlink, through the content cache.
    ///
    /// Exposed for copy-up, which needs every byte rather than a range and
    /// should not defeat the cache by reading around it.
    pub async fn content_bytes(&self, inode: u64) -> Result<Arc<Vec<u8>>> {
        self.content(inode).await
    }

    /// The inode of an entry's parent directory.
    ///
    /// The root is its own parent, as it is on every POSIX filesystem. This
    /// exists so that a client asking for `..` gets the real parent: FUSE and
    /// NFS clients normally resolve `..` from their own caches, but an NFS
    /// client that has lost a dentry sends a LOOKUP for it, and answering
    /// ENOENT there makes `cd ..` fail for no reason the user can see.
    pub fn parent(&self, inode: u64) -> Result<u64> {
        let table = self.inodes.lock().expect("lock is poisoned");
        let entry = table.get(inode).ok_or(SnapshotError::NotFound)?;
        match entry.path.parent() {
            None => Ok(ROOT_INODE),
            // The parent was interned on the way in, since nothing can reach a
            // child without having listed the directory holding it.
            Some(parent) => table
                .by_path
                .get(parent)
                .copied()
                .ok_or(SnapshotError::NotFound),
        }
    }

    /// Size of a file or symlink, in the number of bytes a read will yield.
    ///
    /// A resolved file is sized from the store without touching its content,
    /// which is the difference between an `ls -l` that reads the directory and
    /// one that reads every file in it. Everything else has no size until it is
    /// built: a conflicted path is not bytes at all until its sides are merged
    /// into marker text, and a symlink target is not a stored blob.
    ///
    /// Whatever the route, the answer is exact and never an estimate. A
    /// filesystem that under-reports here makes Nix write truncated content
    /// into its store, addressed under a hash of bytes that were never there,
    /// with no error anywhere. See <https://github.com/NixOS/nix/issues/10667>.
    async fn size(&self, inode: u64, entry: &Inode) -> Result<u64> {
        if let Some(cached) = self
            .contents
            .lock()
            .expect("lock is poisoned")
            .peek(&inode)
            .map(|content| content.len())
        {
            return Ok(u64::try_from(cached).expect("file length fits u64"));
        }
        if let Some(Some(TreeValue::File { id, .. })) = entry.value.as_resolved() {
            return Ok(self.store.file_size(&entry.path, id).await?);
        }
        let content = self.content(inode).await?;
        Ok(u64::try_from(content.len()).expect("file length fits u64"))
    }

    fn inode(&self, inode: u64) -> Result<Inode> {
        let table = self.inodes.lock().expect("lock is poisoned");
        table.get(inode).cloned().ok_or(SnapshotError::NotFound)
    }

    /// Content of a file or symlink, materializing conflicts.
    ///
    /// This buffers the whole object. jj's `Store::read_file` returns a stream
    /// with no length, so a `getattr` cannot report a size without reading, and
    /// a conflicted path has no content at all until its sides are merged into
    /// marker text. Given that we have to read to size, we may as well keep the
    /// bytes; the cache is bounded so a large tree does not grow without limit.
    /// The consequence is that reading one byte of a large file costs the whole
    /// file, which is the price of the backend not exposing blob sizes.
    async fn content(&self, inode: u64) -> Result<Arc<Vec<u8>>> {
        if let Some(hit) = self
            .contents
            .lock()
            .expect("lock is poisoned")
            .get(&inode)
            .cloned()
        {
            return Ok(hit);
        }
        let entry = self.inode(inode)?;
        let bytes = Arc::new(self.materialize(&entry.path, entry.value).await?);
        // A file larger than the whole budget is rejected by the cache rather
        // than evicting everything else to hold it. Serving it uncached is slow
        // but correct, so say so once instead of failing the read.
        let cached = self
            .contents
            .lock()
            .expect("lock is poisoned")
            .put_with_weight(inode, bytes.clone());
        if cached.is_err() {
            tracing::debug!(
                inode,
                len = bytes.len(),
                "file is larger than the content cache; every read will re-read it"
            );
        }
        Ok(bytes)
    }

    async fn materialize(&self, path: &RepoPath, value: MergedTreeValue) -> Result<Vec<u8>> {
        let materialized = materialize_tree_value(&self.store, path, value, &self.labels).await?;
        match materialized {
            MaterializedTreeValue::Absent => Err(SnapshotError::NotFound),
            MaterializedTreeValue::AccessDenied(source) => Err(SnapshotError::AccessDenied {
                path: display_path(path),
                source,
            }),
            MaterializedTreeValue::File(mut file) => Ok(file.read_all(path).await?),
            MaterializedTreeValue::Symlink { target, .. } => Ok(target.into_bytes()),
            // A conflicted file reads back as the same conflict-marker text jj
            // would have written into a working copy. See the crate docs for
            // the alternatives that were rejected.
            MaterializedTreeValue::FileConflict(file) => Ok(materialize_merge_result_to_bytes(
                &file.contents,
                &file.labels,
                &self.materialize,
            )
            .into()),
            // A conflict whose sides are not all files, for example file
            // against symlink, has no marker representation. jj already prints
            // a human summary for this case in `jj file show`, so reuse it
            // rather than inventing a second format.
            MaterializedTreeValue::OtherConflict { id, labels } => {
                Ok(id.describe(&labels).into_bytes())
            }
            // A submodule is presented as an empty directory, so nothing ever
            // asks for its content.
            MaterializedTreeValue::GitSubmodule(_) => Ok(Vec::new()),
            MaterializedTreeValue::Tree(_) => Err(SnapshotError::IsADirectory {
                path: display_path(path),
            }),
        }
    }

    async fn directory(&self, inode: u64) -> Result<Arc<Directory>> {
        if let Some(hit) = self
            .directories
            .lock()
            .expect("lock is poisoned")
            .get(&inode)
            .cloned()
        {
            return Ok(hit);
        }
        let entry = self.inode(inode)?;
        if entry.kind != EntryKind::Directory {
            return Err(SnapshotError::NotADirectory {
                path: display_path(&entry.path),
            });
        }
        // A submodule and a path whose terms are not all trees both land here
        // as None. Presenting an empty directory matches what a git checkout of
        // an uncloned submodule looks like, and keeps the entry's type stable
        // between readdir and getattr.
        let trees = self.root_trees.sub_tree_recursive(&entry.path).await?;
        let listing = match trees {
            Some(trees) => self.build_directory(&entry.path, &trees),
            None => Directory {
                entries: Vec::new(),
                by_name: HashMap::new(),
            },
        };
        let listing = Arc::new(listing);
        self.directories
            .lock()
            .expect("lock is poisoned")
            .insert(inode, listing.clone());
        Ok(listing)
    }

    fn build_directory(&self, dir: &RepoPath, trees: &Merge<Tree>) -> Directory {
        let same_change = self.store.merge_options().same_change;
        let mut entries = Vec::new();
        let mut by_name = HashMap::new();
        for (name, values) in all_merged_tree_entries(trees) {
            // jj-lib's own listing helper applies this before handing values
            // out, but only through a private wrapper (`all_tree_entries`), so
            // repeat it here. Without it a trivially mergeable path inside a
            // conflicted tree would be reported as conflicted.
            let values = match values.resolve_trivial(same_change) {
                Some(resolved) => Merge::resolved(*resolved),
                None => values,
            };
            // A tree can hold a name no filesystem can represent, `.` and `..`
            // being the obvious ones. There is nowhere to put such an entry, so
            // skip it loudly rather than failing the whole listing.
            let fs_name = match name.to_fs_name() {
                Ok(fs_name) => fs_name.to_owned(),
                Err(err) => {
                    tracing::warn!(?err, "skipping tree entry that has no filesystem name");
                    continue;
                }
            };
            let Some((kind, conflicted)) = classify(&values) else {
                continue;
            };
            let path = dir.join(name);
            let inode = self.inodes.lock().expect("lock is poisoned").intern(
                path,
                values.cloned(),
                kind,
                conflicted,
            );
            by_name.insert(fs_name.clone(), entries.len());
            entries.push(TreeEntry {
                inode,
                name: fs_name,
                kind,
                conflicted,
            });
        }
        Directory { entries, by_name }
    }
}

/// Decides what a tree value looks like through the mount.
///
/// Returns `None` for an absent value, which a listing should not contain but
/// which a conflict can produce when a path exists on only some sides.
fn classify(values: &MergedTreeVal<'_>) -> Option<(EntryKind, bool)> {
    if values.is_absent() {
        return None;
    }
    match values.as_resolved() {
        Some(Some(TreeValue::Tree(_))) => Some((EntryKind::Directory, false)),
        Some(Some(TreeValue::File { executable, .. })) => Some((
            EntryKind::File {
                executable: *executable,
            },
            false,
        )),
        Some(Some(TreeValue::Symlink(_))) => Some((EntryKind::Symlink, false)),
        // jj does not populate submodules into a working copy either.
        Some(Some(TreeValue::GitSubmodule(_))) => Some((EntryKind::Directory, false)),
        Some(None) => None,
        None => {
            if values.is_tree() {
                // A conflict between directories is still a directory; the
                // conflict shows up on the paths inside it.
                Some((EntryKind::Directory, true))
            } else {
                // Everything else, including a symlink conflicting with a file,
                // is served as a regular file holding marker text.
                let executable = values
                    .to_executable_merge()
                    .and_then(|merge| resolve_file_executable(&merge))
                    .unwrap_or(false);
                Some((EntryKind::File { executable }, true))
            }
        }
    }
}

fn display_path(path: &RepoPath) -> String {
    if path.is_root() {
        "<root>".to_owned()
    } else {
        path.as_internal_file_string().to_owned()
    }
}

fn timestamp_to_system_time(timestamp: &Timestamp) -> SystemTime {
    let millis = timestamp.timestamp.0;
    if millis >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_millis(millis.unsigned_abs())
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_millis(millis.unsigned_abs())
    }
}

/// Conflict marker options a mount should use by default.
///
/// A mount has no interactive user to ask, so this mirrors what jj writes into
/// a working copy with default settings. A caller that has the user's config in
/// hand should build its own and pass the configured marker style instead.
pub fn default_materialize_options(merge: MergeOptions) -> ConflictMaterializeOptions {
    ConflictMaterializeOptions {
        marker_style: ConflictMarkerStyle::Diff,
        marker_len: None,
        merge,
    }
}
