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

//! FUSE adapter: serves an [`OverlayTree`] to the Linux kernel's FUSE driver.
//!
//! This is the fast path on Linux, one kernel hop with no RPC framing, and it
//! is Linux-only on purpose. macOS FUSE means macFUSE, a third-party kernel
//! extension that needs third-party kexts enabled, so on macOS the NFS adapter
//! is the answer instead.
//!
//! Where a fix to the FUSE layer has to go: `fuser`'s README states that pull
//! requests are no longer accepted, so a bug in the protocol layer cannot be
//! sent upstream. The options are an issue at
//! <https://github.com/cberner/fuser> and a local workaround here, or a fork.
//! Worth knowing before someone spends an afternoon preparing a patch.
//!
//! FUSE's [`fuser::Filesystem`] callbacks are synchronous while the jj store is
//! async, so each callback blocks on its future with `pollster`. That is the
//! same bridge jj-lib uses internally for its own sync entry points. The cost
//! is that one slow read stalls the worker thread handling it, which is why the
//! session is configured with several event loop threads.

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fuser::Errno;
use fuser::FileAttr;
use fuser::FileHandle;
use fuser::FileType;
use fuser::Filesystem;
use fuser::Generation;
use fuser::INodeNo;
use fuser::LockOwner;
use fuser::OpenFlags;
use fuser::ReplyAttr;
use fuser::ReplyData;
use fuser::ReplyDirectory;
use fuser::ReplyEntry;
use fuser::ReplyStatfs;
use fuser::Request;
use pollster::FutureExt as _;

use crate::overlay::OverlayTree;
use crate::snapshot::Attributes;
use crate::snapshot::EntryKind;
use crate::snapshot::SnapshotError;

/// How long the kernel may cache an attribute or a name lookup.
///
/// The tree is immutable for the life of the mount, so nothing we report can go
/// stale and a long TTL is not a tradeoff: it removes almost all repeat
/// traffic. A day rather than a literal eternity only so that a wedged mount is
/// not indistinguishable from a healthy one.
///
/// True only while the mount is read-only, which on this transport it always
/// is: `jj fs mount` refuses `--writable` with `--transport fuse`. Whoever
/// implements the FUSE write path has to shorten this first, since a day-long
/// attribute cache over a tree that changes hands the kernel stale sizes.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Every inode is minted once and never reused, so no inode is ever a recycled
/// one and the kernel never needs a generation to disambiguate.
const GENERATION: Generation = Generation(0);

/// `stat` block size. Anything is legal; 4096 is what callers expect.
const BLOCK_SIZE: u32 = 4096;

/// Serves a jj tree over FUSE.
pub struct FuseTree {
    tree: Arc<OverlayTree>,
    uid: u32,
    gid: u32,
}

impl FuseTree {
    /// Wraps a tree, reporting `uid` and `gid` as the owner of every entry.
    pub fn new(tree: Arc<OverlayTree>, uid: u32, gid: u32) -> Self {
        Self { tree, uid, gid }
    }

    fn file_attr(&self, attributes: &Attributes) -> FileAttr {
        let (kind, nlink) = match attributes.kind {
            EntryKind::Directory => (FileType::Directory, 2),
            EntryKind::File { .. } => (FileType::RegularFile, 1),
            EntryKind::Symlink => (FileType::Symlink, 1),
        };
        // Narrowed rather than cast, so that a future mode carrying a bit above
        // the low twelve fails here instead of silently losing it.
        let perm = u16::try_from(self.tree.mode_bits(attributes.kind))
            .expect("a permission mask fits in u16");
        let time = attributes.mtime;
        FileAttr {
            ino: INodeNo(attributes.inode),
            size: attributes.size,
            blocks: attributes.size.div_ceil(512),
            atime: time,
            mtime: time,
            ctime: time,
            crtime: time,
            kind,
            perm,
            nlink,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: BLOCK_SIZE,
            flags: 0,
        }
    }
}

fn errno(err: &SnapshotError) -> Errno {
    Errno::from_i32(err.errno())
}

fn file_type(kind: EntryKind) -> FileType {
    match kind {
        EntryKind::Directory => FileType::Directory,
        EntryKind::File { .. } => FileType::RegularFile,
        EntryKind::Symlink => FileType::Symlink,
    }
}

impl Filesystem for FuseTree {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        // A jj path is a UTF-8 string, so a non-UTF-8 name cannot name anything
        // in the tree. Report a miss rather than an encoding error, which is
        // what a caller trying such a name is really being told.
        let Some(name) = name.to_str() else {
            reply.error(Errno::ENOENT);
            return;
        };
        let entry = match self.tree.lookup(parent.0, name).block_on() {
            Ok(entry) => entry,
            Err(err) => {
                reply.error(errno(&err));
                return;
            }
        };
        match self.tree.getattr(entry.inode).block_on() {
            Ok(attributes) => reply.entry(&CACHE_TTL, &self.file_attr(&attributes), GENERATION),
            Err(err) => reply.error(errno(&err)),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.tree.getattr(ino.0).block_on() {
            Ok(attributes) => reply.attr(&CACHE_TTL, &self.file_attr(&attributes)),
            Err(err) => reply.error(errno(&err)),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        match self.tree.readlink(ino.0).block_on() {
            Ok(target) => reply.data(&target),
            Err(err) => reply.error(errno(&err)),
        }
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        match self.tree.read(ino.0, offset, size).block_on() {
            Ok((data, _eof)) => reply.data(&data),
            Err(err) => reply.error(errno(&err)),
        }
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let entries = match self.tree.readdir(ino.0).block_on() {
            Ok(entries) => entries,
            Err(err) => {
                reply.error(errno(&err));
                return;
            }
        };
        // "." and ".." come first so that the offset arithmetic below is a plain
        // index into one synthetic list. The kernel needs both present; a
        // directory listing without them makes some tools decide the directory
        // is unreadable.
        let parent = self.tree.parent(ino.0).unwrap_or(ino.0);
        let synthetic = [
            (ino.0, FileType::Directory, "."),
            (parent, FileType::Directory, ".."),
        ];
        let all = synthetic.into_iter().chain(
            entries
                .iter()
                .map(|entry| (entry.inode, file_type(entry.kind), entry.name.as_str())),
        );
        // The offset is the "next entry" cursor the kernel hands back from a
        // previous call, so entries already delivered are skipped rather than
        // resent.
        for (index, (inode, kind, name)) in all.enumerate().skip(offset as usize) {
            let next = u64::try_from(index).expect("entry index fits u64") + 1;
            if reply.add(INodeNo(inode), next, kind, name) {
                // Buffer full. Stop here; the kernel asks again from `next`.
                break;
            }
        }
        reply.ok();
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        // A jj tree has no block accounting and nothing can be written, so the
        // only honest answer is a full filesystem with no free space. `df`
        // showing 100% used is correct here rather than a placeholder.
        reply.statfs(0, 0, 0, 0, 0, BLOCK_SIZE, 255, BLOCK_SIZE);
    }
}

/// A live FUSE mount.
///
/// The mount exists from [`Self::new`] until this value is dropped or
/// [`Unmounter::unmount`] is called, so a caller cannot accidentally leave the
/// kernel talking to a filesystem that has gone away.
pub struct Mount {
    session: fuser::Session<FuseTree>,
}

impl Mount {
    /// Mounts `tree` at `mountpoint`, reporting `uid` and `gid` as the
    /// owner of every entry.
    pub fn new(tree: Arc<OverlayTree>, mountpoint: &Path, uid: u32, gid: u32) -> io::Result<Self> {
        // fuser::Config is #[non_exhaustive], so it is filled in from Default
        // rather than built literally. That is the right shape anyway: a new
        // option upstream adds a field we have not considered, and taking its
        // default is better than failing to compile into a hand-written literal.
        let mut config = fuser::Config::default();
        config.mount_options = vec![
            // Read-only, no setuid and no device nodes are stated at the mount as
            // well as in each file's mode, so a caller that checks the mount
            // flags rather than the modes gets the same answer. This transport
            // implements no write callbacks at all and `jj fs mount` refuses
            // `--writable` here, so RO states a fact rather than a policy.
            fuser::MountOption::RO,
            fuser::MountOption::NoSuid,
            fuser::MountOption::NoDev,
            fuser::MountOption::FSName("jj".to_owned()),
            fuser::MountOption::Subtype("jj".to_owned()),
            // Without this, FUSE delegates permission checks to the filesystem
            // rather than doing them itself, and since this filesystem does not
            // implement `access`, the kernel grants everything. The visible
            // symptom is that `test -x` succeeds on a mode 0444 file, so a
            // caller deciding whether something is runnable gets the wrong
            // answer. The modes reported here are accurate, so having the kernel
            // enforce them is both correct and less code than an `access`
            // callback that would reimplement the same check.
            fuser::MountOption::DefaultPermissions,
        ];
        // Only the mounting user can see the mount. Widening this to AllowOther
        // would expose a revision's contents to every local user.
        config.acl = fuser::SessionACL::Owner;
        // Each callback blocks on a store read, so a single event loop thread
        // would serialize the whole mount behind one slow file.
        config.n_threads = Some(4);
        config.clone_fd = true;
        let session = fuser::Session::new(FuseTree::new(tree, uid, gid), mountpoint, &config)?;
        Ok(Self { session })
    }

    /// A handle that can unmount from another thread or from a signal handler.
    pub fn unmounter(&mut self) -> Unmounter {
        Unmounter(self.session.unmount_callable())
    }

    /// Serves requests until the filesystem is unmounted, by an [`Unmounter`]
    /// or by anything else that unmounts it.
    pub fn run(self) -> io::Result<()> {
        self.session.run()
    }
}

/// Unmounts a [`Mount`] from outside the thread serving it.
pub struct Unmounter(fuser::SessionUnmounter);

impl Unmounter {
    /// Unmounts. Returns an error if the mount is already gone, which is the
    /// normal case when the session ended on its own.
    pub fn unmount(&mut self) -> io::Result<()> {
        self.0.unmount()
    }
}
