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

//! NFSv3 adapter: serves a [`TreeSnapshot`] to the operating system's own NFS
//! client over a loopback TCP socket.
//!
//! This exists because it is the only transport that reaches macOS without a
//! kernel extension. macOS has no in-kernel FUSE; macFUSE is a third-party kext
//! needing a reduced security posture, which jj cannot require of a user. Every
//! platform jj supports does ship an NFS client in its kernel, so serving NFSv3
//! on 127.0.0.1 and letting the kernel mount it needs nothing installed. Meta's
//! EdenFS reaches macOS the same way.
//!
//! Unlike EdenFS this needs no privileged helper. macOS lets an unprivileged
//! user mount NFS from localhost as long as the client is not asked for a
//! reserved source port, so `noresvport` and a port above 1024 keep the whole
//! thing inside one unprivileged process.

use std::io;
use std::sync::Arc;

use nfs3_server::nfs3_types::nfs3::Nfs3Option;
use nfs3_server::nfs3_types::nfs3::cookie3;
use nfs3_server::nfs3_types::nfs3::createverf3;
use nfs3_server::nfs3_types::nfs3::entry3;
use nfs3_server::nfs3_types::nfs3::fattr3;
use nfs3_server::nfs3_types::nfs3::filename3;
use nfs3_server::nfs3_types::nfs3::ftype3;
use nfs3_server::nfs3_types::nfs3::nfspath3;
use nfs3_server::nfs3_types::nfs3::nfsstat3;
use nfs3_server::nfs3_types::nfs3::nfstime3;
use nfs3_server::nfs3_types::nfs3::sattr3;
use nfs3_server::nfs3_types::nfs3::specdata3;
use nfs3_server::nfs3_types::nfs3::stable_how;
use nfs3_server::tcp::NFSTcp as _;
use nfs3_server::tcp::NFSTcpListener;
use nfs3_server::vfs::DirEntry;
use nfs3_server::vfs::DirEntryPlus;
use nfs3_server::vfs::FileHandleU64;
use nfs3_server::vfs::NextResult;
use nfs3_server::vfs::NfsFileSystem;
use nfs3_server::vfs::NfsReadFileSystem;
use nfs3_server::vfs::ReadDirIterator;
use nfs3_server::vfs::ReadDirPlusIterator;
use nfs3_server::vfs::VFSCapabilities;

use crate::overlay::OverlayTree;
use crate::snapshot::Attributes;
use crate::snapshot::EntryKind;
use crate::snapshot::ROOT_INODE;
use crate::snapshot::SnapshotError;
use crate::snapshot::TreeEntry;
use crate::snapshot::TreeSnapshot;
use crate::stats::Op;
use crate::stats::Stats;
use crate::stats::Timer;

/// High bits of the reported fsid, so that a jj mount is recognizable in
/// `stat` output rather than looking like a low-numbered real device.
const FSID_TAG: u64 = 0x6a6a_0000_0000_0000;

/// Serves a jj tree over NFSv3.
pub struct NfsTree {
    tree: Arc<OverlayTree>,
    fsid: u64,
    uid: u32,
    gid: u32,
    stats: Arc<Stats>,
}

impl NfsTree {
    /// Wraps a read-only snapshot. `fsid` distinguishes this export from any
    /// other on the same host; `uid` and `gid` are reported as the owner of
    /// every entry, normally the calling user so that the mount looks like
    /// their own files.
    pub fn new(snapshot: Arc<TreeSnapshot>, fsid: u64, uid: u32, gid: u32) -> Self {
        Self::with_tree(Arc::new(OverlayTree::read_only(snapshot)), fsid, uid, gid)
    }

    /// Wraps a tree that may have a writable overlay.
    pub fn with_tree(tree: Arc<OverlayTree>, fsid: u64, uid: u32, gid: u32) -> Self {
        Self {
            tree,
            fsid,
            uid,
            gid,
            stats: Arc::new(Stats::new()),
        }
    }

    /// Per-procedure counts and in-handler time for this mount.
    pub fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }

    /// Starts timing one procedure.
    fn timed(&self, op: Op) -> Timer<'_> {
        Timer::new(&self.stats, op)
    }

    fn fattr(&self, attributes: &Attributes) -> fattr3 {
        self.fattr_context().fattr(attributes)
    }

    fn fattr_context(&self) -> FattrContext {
        FattrContext {
            tree: self.tree.clone(),
            fsid: self.fsid,
            uid: self.uid,
            gid: self.gid,
        }
    }
}

/// What building a `fattr3` needs beyond one item's own attributes.
///
/// Split out of [`NfsTree`] so a directory iterator can size its entries
/// without borrowing the tree it came from. It carries the [`OverlayTree`]
/// because both the mode bits and the attributes themselves come from there.
#[derive(Clone)]
struct FattrContext {
    tree: Arc<OverlayTree>,
    fsid: u64,
    uid: u32,
    gid: u32,
}

impl FattrContext {
    fn fattr(&self, attributes: &Attributes) -> fattr3 {
        let (type_, nlink) = match attributes.kind {
            // nlink 2 is the conventional answer for a directory whose
            // subdirectory count we do not want to compute; `find` and `du`
            // both cope with it.
            EntryKind::Directory => (ftype3::NF3DIR, 2),
            EntryKind::File { .. } => (ftype3::NF3REG, 1),
            EntryKind::Symlink => (ftype3::NF3LNK, 1),
        };
        // Per entry rather than per mount: an overlay serves real files whose
        // timestamps move, and a build system that is told a file it wrote
        // thirty seconds ago is as old as the commit will skip work it has to
        // do.
        let time = nfstime3::try_from(attributes.mtime).unwrap_or(nfstime3 {
            seconds: 0,
            nseconds: 0,
        });
        fattr3 {
            type_,
            mode: self.tree.mode_bits(attributes.kind),
            nlink,
            uid: self.uid,
            gid: self.gid,
            size: attributes.size,
            used: attributes.size,
            rdev: specdata3 {
                specdata1: 0,
                specdata2: 0,
            },
            fsid: self.fsid,
            fileid: attributes.inode,
            atime: time,
            mtime: time,
            ctime: time,
        }
    }
}

impl NfsTree {
    /// Attributes of an inode, as the protocol wants them.
    async fn fattr_of(&self, inode: u64) -> Result<fattr3, nfsstat3> {
        let attributes = self
            .tree
            .getattr(inode)
            .await
            .map_err(|err| nfs_status(&err))?;
        Ok(self.fattr(&attributes))
    }
}

/// An NFSv3 name as a `str`.
///
/// NFSv3 names are opaque bytes, but a jj path is a UTF-8 string, so a name
/// that is not UTF-8 cannot exist in either layer.
fn name_str<'a>(filename: &'a filename3<'_>) -> Result<&'a str, nfsstat3> {
    std::str::from_utf8(filename.0.as_ref()).map_err(|_| nfsstat3::NFS3ERR_NOENT)
}

/// Maps a core error onto the closest NFSv3 status, saying so when a write was
/// refused.
///
/// A client turns `ROFS` into "Read-only file system" and nothing else. On a
/// mount that is mostly writable that message is actively misleading, and the
/// user has no way to find out which path was refused or why. This is the only
/// place that knows, so it is the only place that can say. `JJ_LOG=warn` on the
/// serving process turns it on.
fn nfs_status(err: &SnapshotError) -> nfsstat3 {
    match err {
        SnapshotError::NotFound => nfsstat3::NFS3ERR_NOENT,
        SnapshotError::NotADirectory { .. } => nfsstat3::NFS3ERR_NOTDIR,
        SnapshotError::NotASymlink { .. } => nfsstat3::NFS3ERR_INVAL,
        SnapshotError::IsADirectory { .. } => nfsstat3::NFS3ERR_ISDIR,
        SnapshotError::InvalidName { .. } => nfsstat3::NFS3ERR_INVAL,
        SnapshotError::AccessDenied { .. } => nfsstat3::NFS3ERR_ACCES,
        // ROFS rather than the access or permission statuses. A client that sees
        // ROFS reports "read only file system" and stops, where a permission
        // status sends the user looking for a problem that does not exist.
        SnapshotError::ReadOnly | SnapshotError::Tracked { .. } => {
            tracing::warn!(%err, "refused a write");
            nfsstat3::NFS3ERR_ROFS
        }
        SnapshotError::Exists { .. } => nfsstat3::NFS3ERR_EXIST,
        SnapshotError::NotEmpty { .. } => nfsstat3::NFS3ERR_NOTEMPTY,
        SnapshotError::Unsupported { .. } => nfsstat3::NFS3ERR_NOTSUPP,
        SnapshotError::OverlayBusy { .. } => nfsstat3::NFS3ERR_IO,
        SnapshotError::Io { .. } => nfsstat3::NFS3ERR_IO,
        SnapshotError::Backend(_) | SnapshotError::Materialize { .. } => nfsstat3::NFS3ERR_IO,
    }
}

impl NfsReadFileSystem for NfsTree {
    type Handle = FileHandleU64;

    fn root_dir(&self) -> FileHandleU64 {
        FileHandleU64::new(ROOT_INODE)
    }

    async fn lookup(
        &self,
        dirid: &FileHandleU64,
        filename: &filename3<'_>,
    ) -> Result<FileHandleU64, nfsstat3> {
        // NFSv3 names are opaque bytes. A name that is not UTF-8 cannot be in a
        // jj tree, whose paths are UTF-8 strings, so it can only be a miss.
        let _timer = self.timed(Op::Lookup);
        let name = name_str(filename)?;
        // A client normally resolves these from its own cache, but an NFS client
        // that has lost a dentry asks for ".." by name, and answering ENOENT
        // there makes `cd ..` fail for no reason the user can see.
        match name {
            "." => return Ok(*dirid),
            ".." => {
                let parent = self
                    .tree
                    .parent(dirid.as_u64())
                    .map_err(|err| nfs_status(&err))?;
                return Ok(FileHandleU64::new(parent));
            }
            _ => {}
        }
        let entry = self
            .tree
            .lookup(dirid.as_u64(), name)
            .await
            .map_err(|err| nfs_status(&err))?;
        Ok(FileHandleU64::new(entry.inode))
    }

    async fn getattr(&self, id: &FileHandleU64) -> Result<fattr3, nfsstat3> {
        let _timer = self.timed(Op::Getattr);
        self.fattr_of(id.as_u64()).await
    }

    async fn read(
        &self,
        id: &FileHandleU64,
        offset: u64,
        count: u32,
    ) -> Result<(Vec<u8>, bool), nfsstat3> {
        let mut timer = self.timed(Op::Read);
        let result = self
            .tree
            .read(id.as_u64(), offset, count)
            .await
            .map_err(|err| nfs_status(&err));
        if let Ok((data, _)) = &result {
            timer.bytes(u64::try_from(data.len()).unwrap_or(0));
        }
        result
    }

    async fn readdir(
        &self,
        dirid: &FileHandleU64,
        cookie: u64,
    ) -> Result<impl ReadDirIterator, nfsstat3> {
        let _timer = self.timed(Op::Readdir);
        Ok(NamesOnly(self.dir_iterator(dirid, cookie).await?))
    }

    async fn readdirplus(
        &self,
        dirid: &FileHandleU64,
        cookie: u64,
    ) -> Result<impl ReadDirPlusIterator<FileHandleU64>, nfsstat3> {
        let _timer = self.timed(Op::Readdir);
        self.dir_iterator(dirid, cookie).await
    }

    async fn readlink(&self, id: &FileHandleU64) -> Result<nfspath3<'_>, nfsstat3> {
        let _timer = self.timed(Op::Readlink);
        let target = self
            .tree
            .readlink(id.as_u64())
            .await
            .map_err(|err| nfs_status(&err))?;
        Ok(nfspath3::from(target))
    }
}

impl NfsTree {
    async fn dir_iterator(
        &self,
        dirid: &FileHandleU64,
        cookie: cookie3,
    ) -> Result<DirIterator, nfsstat3> {
        let mut entries = self
            .tree
            .readdir(dirid.as_u64())
            .await
            .map_err(|err| nfs_status(&err))?;
        // The cookie is one past the index of the last entry the client has, in
        // a listing ordered by name. Position rather than fileid, and never
        // BAD_COOKIE, because with an overlay a directory is mutable and the
        // entry a fileid cookie named can be gone by the next page.
        //
        // nfs3_server reached the same conclusion about its own cookieverf
        // check and disabled it: see the comment at `nfs_handlers.rs`, which
        // records that returning BAD_COOKIE makes the macOS client fail a
        // listing with "no such file or directory". A cookie we cannot honor
        // exactly is better answered approximately than rejected.
        //
        // The cost is that a directory modified between two pages of one
        // listing can skip or repeat an entry. That needs a directory large
        // enough to paginate, which is tens of thousands of names, and a write
        // landing inside the same listing. Both are possible; neither is an
        // error, which is the trade being made.
        let skip = usize::try_from(cookie)
            .unwrap_or(usize::MAX)
            .min(entries.len());
        // Draining rather than slicing keeps the listing owned, so nothing is
        // cloned to build the iterator.
        entries.drain(..skip);
        Ok(DirIterator {
            entries: entries.into_iter(),
            next_cookie: cookie + 1,
            fattr: self.fattr_context(),
        })
    }
}

/// Yields the remaining entries of one directory listing.
struct DirIterator {
    entries: std::vec::IntoIter<TreeEntry>,
    /// Position to hand the client for the entry about to be yielded.
    next_cookie: cookie3,
    fattr: FattrContext,
}

impl DirIterator {
    /// The next entry, with no attribute work done for it.
    ///
    /// `readdir` and `readdirplus` read one listing but should not pay one
    /// cost: only the plus form has anywhere to put an attribute.
    fn next_entry(&mut self) -> Option<(TreeEntry, cookie3)> {
        let entry = self.entries.next()?;
        let cookie = self.next_cookie;
        self.next_cookie += 1;
        Some((entry, cookie))
    }
}

impl ReadDirPlusIterator<FileHandleU64> for DirIterator {
    /// Inlines attributes for the entries we can size without reading content.
    ///
    /// This returned `None` for every entry until now, and that was the right
    /// trade when it was written: sizing a file meant reading it, so filling
    /// these in would have turned listing a directory into reading every file
    /// in it. The premise expired underneath it. `Backend::file_size` answers
    /// from the git object header -- a loose object carries its length in the
    /// first bytes of its zlib stream, a packed one in its pack entry header --
    /// so neither is inflated, and `getattr` already takes that path.
    ///
    /// The trade now runs the other way. Omitting attributes costs the client
    /// one LOOKUP per entry it cares about, measured at 63.9us of round trip,
    /// against a local object-header read well under 1us. On a cold walk of a
    /// 21,196-entry tree that was 22,477 LOOKUPs, 72% of every RPC the walk
    /// made. It also leaves `d_type` unset, so a caller that would have used
    /// the dirent hint to avoid a stat cannot, and stats anyway.
    async fn next(&mut self) -> NextResult<DirEntryPlus<FileHandleU64>> {
        let Some((entry, cookie)) = self.next_entry() else {
            return NextResult::Eof;
        };
        // NFSv3 makes these optional per entry, so each case decides alone.
        let name_attributes = match (entry.conflicted, entry.kind) {
            // The one case the original trade still protects. A conflicted path
            // has no content until its sides are materialized into marker text,
            // so sizing it here would do exactly the work this used to warn
            // about. Leave it to a LOOKUP.
            (true, _) => None,
            // Same reason: the tree reveals a link's target, and so its length,
            // only by handing the target over.
            (false, EntryKind::Symlink) => None,
            // Both cheap. A directory reports a constant, and a file's length
            // comes from the object header, or from a scratch-layer stat when
            // the overlay owns it.
            (false, EntryKind::Directory | EntryKind::File { .. }) => {
                match self.fattr.tree.getattr(entry.inode).await {
                    Ok(attributes) => Some(self.fattr.fattr(&attributes)),
                    // Attributes are an optimization, so failing to size one
                    // entry must not fail the listing it appears in.
                    Err(_) => None,
                }
            }
        };
        NextResult::Ok(DirEntryPlus {
            fileid: entry.inode,
            name: filename3::from(entry.name.into_bytes()),
            cookie,
            name_attributes,
            name_handle: Some(FileHandleU64::new(entry.inode)),
        })
    }
}

/// The writable half.
///
/// Every method delegates to [`OverlayTree`], which decides whether the
/// operation is allowed. Nothing here has a policy of its own: a mount with no
/// overlay reaches these methods and gets `ROFS` from the core, which is why
/// there is no second read-only implementation to keep in step.
impl NfsFileSystem for NfsTree {
    fn capabilities(&self) -> VFSCapabilities {
        if self.tree.is_writable() {
            VFSCapabilities::ReadWrite
        } else {
            VFSCapabilities::ReadOnly
        }
    }

    async fn setattr(&self, id: &FileHandleU64, setattr: sattr3) -> Result<fattr3, nfsstat3> {
        let _timer = self.timed(Op::Setattr);
        let inode = id.as_u64();
        // Order matters: a client that truncates and chmods in one SETATTR
        // expects both applied, and applying the size first means the mode is
        // the one the caller asked for rather than the one copy-up chose.
        if let Nfs3Option::Some(size) = setattr.size {
            self.tree
                .truncate(inode, size)
                .await
                .map_err(|err| nfs_status(&err))?;
        }
        if let Nfs3Option::Some(mode) = setattr.mode {
            self.tree
                .chmod(inode, mode)
                .await
                .map_err(|err| nfs_status(&err))?;
        }
        // uid, gid and the timestamps are accepted and ignored. The mount is
        // served entirely as the calling user, so there is no other owner to
        // set, and refusing would make `cp -p` and `tar -x` fail on a mount
        // where the copy itself worked.
        let _ = (&setattr.uid, &setattr.gid, setattr.atime, setattr.mtime);
        self.fattr_of(inode).await
    }

    async fn write(
        &self,
        id: &FileHandleU64,
        offset: u64,
        data: &[u8],
        _stable: stable_how,
    ) -> Result<(fattr3, stable_how), nfsstat3> {
        let mut timer = self.timed(Op::Write);
        timer.bytes(u64::try_from(data.len()).unwrap_or(0));
        self.tree
            .write(id.as_u64(), offset, data)
            .await
            .map_err(|err| nfs_status(&err))?;
        let attributes = self.fattr_of(id.as_u64()).await?;
        // FILE_SYNC whatever was asked for, because the write really did reach
        // the host filesystem before this returned. Claiming less would invite
        // the client to send a COMMIT that has nothing to do.
        Ok((attributes, stable_how::FILE_SYNC))
    }

    async fn create(
        &self,
        dirid: &FileHandleU64,
        filename: &filename3<'_>,
        attr: sattr3,
    ) -> Result<(FileHandleU64, fattr3), nfsstat3> {
        let _timer = self.timed(Op::Create);
        let name = name_str(filename)?;
        let mode = match attr.mode {
            Nfs3Option::Some(mode) => Some(mode),
            Nfs3Option::None => None,
        };
        let entry = self
            .tree
            .create(dirid.as_u64(), name, mode)
            .await
            .map_err(|err| nfs_status(&err))?;
        if let Nfs3Option::Some(size) = attr.size {
            self.tree
                .truncate(entry.inode, size)
                .await
                .map_err(|err| nfs_status(&err))?;
        }
        let attributes = self.fattr_of(entry.inode).await?;
        Ok((FileHandleU64::new(entry.inode), attributes))
    }

    async fn create_exclusive(
        &self,
        dirid: &FileHandleU64,
        filename: &filename3<'_>,
        _createverf: createverf3,
    ) -> Result<FileHandleU64, nfsstat3> {
        let name = name_str(filename)?;
        // Exclusive create in NFSv3 means the server persists the client's
        // verifier so that a retried request is recognized as the same one
        // rather than as a second create. There is nowhere to persist it here,
        // and the RFC's answer for a server that cannot is NOTSUPP rather than
        // pretending. Clients fall back to a GUARDED create.
        let _ = (dirid, name);
        Err(nfsstat3::NFS3ERR_NOTSUPP)
    }

    async fn mkdir(
        &self,
        dirid: &FileHandleU64,
        dirname: &filename3<'_>,
    ) -> Result<(FileHandleU64, fattr3), nfsstat3> {
        let _timer = self.timed(Op::Mkdir);
        let name = name_str(dirname)?;
        let entry = self
            .tree
            .mkdir(dirid.as_u64(), name)
            .await
            .map_err(|err| nfs_status(&err))?;
        let attributes = self.fattr_of(entry.inode).await?;
        Ok((FileHandleU64::new(entry.inode), attributes))
    }

    async fn remove(
        &self,
        dirid: &FileHandleU64,
        filename: &filename3<'_>,
    ) -> Result<(), nfsstat3> {
        let _timer = self.timed(Op::Remove);
        let name = name_str(filename)?;
        self.tree
            .remove(dirid.as_u64(), name)
            .await
            .map_err(|err| nfs_status(&err))
    }

    async fn rename<'a>(
        &self,
        from_dirid: &FileHandleU64,
        from_filename: &filename3<'a>,
        to_dirid: &FileHandleU64,
        to_filename: &filename3<'a>,
    ) -> Result<(), nfsstat3> {
        let _timer = self.timed(Op::Rename);
        let from = name_str(from_filename)?;
        let to = name_str(to_filename)?;
        self.tree
            .rename(from_dirid.as_u64(), from, to_dirid.as_u64(), to)
            .await
            .map_err(|err| nfs_status(&err))
    }

    async fn symlink<'a>(
        &self,
        dirid: &FileHandleU64,
        linkname: &filename3<'a>,
        symlink: &nfspath3<'a>,
        _attr: &sattr3,
    ) -> Result<(FileHandleU64, fattr3), nfsstat3> {
        let _timer = self.timed(Op::Symlink);
        let name = name_str(linkname)?;
        let entry = self
            .tree
            .symlink(dirid.as_u64(), name, symlink.0.as_ref())
            .await
            .map_err(|err| nfs_status(&err))?;
        let attributes = self.fattr_of(entry.inode).await?;
        Ok((FileHandleU64::new(entry.inode), attributes))
    }

    async fn commit(&self, id: &FileHandleU64, _offset: u64, _count: u32) -> Result<(), nfsstat3> {
        let _timer = self.timed(Op::Commit);
        // Every write above reached the host filesystem before it returned, so
        // there is nothing buffered to flush. Confirming the handle exists is
        // still worth doing: a COMMIT against a handle we do not know is a bug
        // somewhere, and answering OK would hide it.
        self.tree
            .getattr(id.as_u64())
            .await
            .map(|_| ())
            .map_err(|err| nfs_status(&err))
    }
}

/// Drops the plus part of a `readdirplus` listing.
///
/// nfs3_server ships an adapter that does this, but it wraps the iterator in a
/// type parameterized over the handle, and going through our own two-line
/// version keeps `readdir` and `readdirplus` reading the same listing.
struct NamesOnly(DirIterator);

impl ReadDirIterator for NamesOnly {
    async fn next(&mut self) -> NextResult<DirEntry> {
        // Deliberately not the plus iterator's `next`: routing through it would
        // size every entry only to drop the answer on the floor.
        match self.0.next_entry() {
            Some((entry, cookie)) => NextResult::Ok(entry3 {
                fileid: entry.inode,
                name: filename3::from(entry.name.into_bytes()),
                cookie,
            }),
            None => NextResult::Eof,
        }
    }
}

/// A running loopback NFSv3 server.
///
/// The server owns its own runtime, so a caller does not need one. Dropping
/// this value stops serving, which is why it has to outlive the mount that
/// points at it: an NFS mount whose server has gone away blocks every syscall
/// against it until the client's timeout rather than failing.
pub struct Server {
    port: u16,
    stats: Arc<Stats>,
    tree_stats: Arc<Stats>,
    // Field order matters only in that the runtime must not be dropped before
    // the tasks it owns; keeping it here is what keeps them alive at all.
    _runtime: tokio::runtime::Runtime,
}

impl Server {
    /// Binds to `127.0.0.1:port` and starts serving. Pass port 0 to let the
    /// operating system pick a free one, then read it back with [`Self::port`].
    ///
    /// Only the loopback address is ever used. An NFSv3 server has no
    /// authentication, so binding anywhere else would publish the revision to
    /// the network.
    ///
    /// A tree with a writable overlay is bound read-write, one without is bound
    /// read-only. The distinction is made here rather than left to the core's
    /// `ROFS` replies because `bind_ro` refuses the write procedures at the RPC
    /// layer, one hop earlier, so a read-only mount cannot reach the write path
    /// at all even if a later change put a bug in it.
    pub fn start(tree: Arc<OverlayTree>, port: u16, uid: u32, gid: u32) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        // The fsid only has to distinguish this export from any other on the
        // host, and the port already does, since two servers cannot share one.
        let fsid = u64::from(port) | FSID_TAG;
        let writable = tree.is_writable();
        let address = format!("127.0.0.1:{port}");
        let tree_stats = tree.stats();
        let served = NfsTree::with_tree(tree, fsid, uid, gid);
        let stats = served.stats();
        // The two binds produce different listener types, because `bind_ro`
        // wraps the filesystem in an adapter that refuses the write procedures.
        // That is the point of using it, so the branch is over the whole
        // bind-and-spawn rather than over the bind alone.
        let port = if writable {
            let listener = runtime.block_on(NFSTcpListener::bind(&address, served))?;
            serve_forever(&runtime, listener)
        } else {
            let listener = runtime.block_on(NFSTcpListener::bind_ro(&address, served))?;
            serve_forever(&runtime, listener)
        };
        Ok(Self {
            port,
            stats,
            tree_stats,
            _runtime: runtime,
        })
    }

    /// Per-procedure counts and in-handler time.
    ///
    /// Two sets because they answer different questions. The transport counters
    /// say how many round trips the client made and how long we held each one;
    /// the tree counters say what the overlay did on its own account, which is
    /// copy-up and discarded sidecars. Wall clock minus the transport total is
    /// everything that is not us.
    pub fn report(&self) -> String {
        format!(
            "NFS procedures:\n{}\noverlay work:\n{}",
            self.stats.report(),
            self.tree_stats.report()
        )
    }

    /// The port actually bound, which differs from the requested one when 0 was
    /// asked for.
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Hands a bound listener to the runtime and reports the port it took.
///
/// Generic because the read-only and read-write binds return different
/// listener types; everything after the bind is identical.
fn serve_forever<T>(runtime: &tokio::runtime::Runtime, listener: NFSTcpListener<T>) -> u16
where
    T: NfsFileSystem + 'static,
{
    let port = listener.get_listen_port();
    runtime.spawn(async move {
        if let Err(err) = listener.handle_forever().await {
            tracing::error!(?err, "NFS server stopped");
        }
    });
    port
}
