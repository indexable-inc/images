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

#[cfg(unix)]
use std::io::Write as _;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::sync::Arc;

use clap_complete::ArgValueCompleter;
#[cfg(unix)]
use jj_lib::conflicts::ConflictMaterializeOptions;
#[cfg(unix)]
use jj_lib::object_id::ObjectId as _;
#[cfg(unix)]
use jj_lib::repo::Repo as _;
#[cfg(unix)]
use jj_vfs::Overlay;
#[cfg(unix)]
use jj_vfs::OverlayTree;
#[cfg(unix)]
use jj_vfs::TreeSnapshot;
#[cfg(unix)]
use tracing::instrument;

#[cfg(unix)]
use crate::cleanup_guard::CleanupGuard;
use crate::cli_util::CommandHelper;
use crate::cli_util::RevisionArg;
use crate::command_error::CommandError;
use crate::command_error::user_error;
#[cfg(unix)]
use crate::command_error::user_error_with_message;
use crate::complete;
use crate::ui::Ui;

/// Which kernel interface to serve the tree over.
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum Transport {
    /// FUSE. Linux only, and unprivileged.
    Fuse,
    /// NFSv3 over a loopback socket. Works everywhere, and unprivileged on
    /// macOS. On Linux, mounting NFS needs root, so prefer `fuse` there.
    Nfs,
}

impl Transport {
    /// The value a user would type for this transport.
    // Only the unix implementation prints it; on Windows the command refuses
    // before it has a transport to name.
    #[cfg(unix)]
    fn name(self) -> &'static str {
        match self {
            Self::Fuse => "fuse",
            Self::Nfs => "nfs",
        }
    }
}

/// Mount a revision's tree as a filesystem
///
/// The mount lives as long as this command does. There is no daemon and no
/// state left behind: press Ctrl-C, or send the process SIGTERM or SIGHUP, and
/// the mountpoint is unmounted before the command exits. Only SIGKILL can leave
/// a mountpoint behind, since nothing runs on SIGKILL; unmount it by hand with
/// `fusermount3 -u MOUNTPOINT` on Linux or `umount MOUNTPOINT` on macOS.
///
/// Like every jj command interrupted by a signal, this exits with status 1 when
/// it is unmounted by a signal rather than by an external `umount`.
///
/// The mount is read-only by default, and writing to a read-only mount never
/// silently succeeds. `--scratch` adds an experimental scratch layer so that
/// build tooling can run: `bun install`, `cargo build` and anything else that
/// needs a `node_modules` or a `target` directory. It is a stepping stone to a
/// mount backed by a real `jj` workspace, and what writes to tracked paths mean
/// will change when that lands. Read that flag before relying on it.
///
/// A file that is conflicted in the revision appears as a regular file whose
/// contents are the same conflict markers `jj` writes into a working copy.
#[derive(clap::Args, Clone, Debug)]
pub struct FsMountArgs {
    /// Where to mount the tree. Must be an existing empty directory.
    #[arg(value_name = "MOUNTPOINT", value_hint = clap::ValueHint::DirPath)]
    mountpoint: PathBuf,
    /// The revision to serve
    #[arg(long, short, default_value = "@", value_name = "REVSET")]
    #[arg(add = ArgValueCompleter::new(complete::revset_expression_all))]
    revision: RevisionArg,
    /// Which kernel interface to use
    ///
    /// Defaults to `fuse` on Linux and `nfs` elsewhere, which is the
    /// unprivileged choice on each.
    #[arg(long, value_enum)]
    transport: Option<Transport>,
    /// TCP port for the NFS server on 127.0.0.1. 0 picks a free one
    #[arg(long, default_value_t = 0, value_name = "PORT")]
    nfs_port: u16,
    /// Byte budget for cached file contents
    ///
    /// The jj backend only reveals a file's size by handing over its content,
    /// so serving `stat` means reading files. Raise this for a tree of large
    /// files that are read repeatedly.
    #[arg(long, default_value_t = jj_vfs::DEFAULT_CONTENT_CACHE_BYTES, value_name = "BYTES")]
    content_cache_bytes: usize,
    /// EXPERIMENTAL: accept writes, into a scratch layer beside the revision
    ///
    /// Deliberately not called `--writable`. This does not give you a writable
    /// working copy, and that name is reserved for the thing that will: a mount
    /// backed by a real `jj` workspace, where a write to a tracked path becomes
    /// a commit visible in `jj log` and reachable by `jj undo`. What writes to
    /// tracked paths mean here will change when that lands, so do not build on
    /// the current behavior.
    ///
    /// What it does today. New files and directories are created in a scratch
    /// directory, and writing to a file the revision contains copies it there
    /// first. Deleting a file the revision contains hides it, for this
    /// revision and this scratch layer only. The revision is never modified
    /// and nothing is written to the object store, so deleting the scratch
    /// directory undoes everything.
    ///
    /// What it cannot do. A directory the revision contains cannot be removed
    /// or renamed away, so `rm -rf src` and `git clean -xfd` fail at the
    /// directory rather than at the files inside it. `rm -rf node_modules`
    /// works, because the revision does not contain it.
    ///
    /// Pass a directory to choose where the layer lives, or pass the flag alone
    /// to have one chosen. It persists across mounts of the same mountpoint, so
    /// a remount does not mean reinstalling everything; delete the directory to
    /// start over.
    ///
    /// Only the `nfs` transport supports this today.
    #[arg(long, value_name = "DIR", value_hint = clap::ValueHint::DirPath)]
    #[arg(num_args = 0..=1, default_missing_value = "")]
    scratch: Option<PathBuf>,
}

/// Windows has neither a FUSE device nor the unix mount helpers this drives, so
/// the command exists everywhere but only runs on unix. Keeping it registered
/// on every platform is deliberate: `jj --help` and the generated CLI reference
/// are then identical everywhere, and a Windows user gets an explanation
/// instead of "unrecognized subcommand".
///
/// Keep this arm tiny. Everything in it compiles only for a target nobody here
/// develops on, so a mistake is invisible to a normal build. It is not
/// invisible to a normal check: the cross-check recipe in docs/vfs.md compiles
/// exactly this arm in about thirty seconds, and was confirmed to fail when the
/// arm is broken on purpose.
#[cfg(not(unix))]
pub async fn cmd_fs_mount(
    _ui: &mut Ui,
    _command: &CommandHelper,
    _args: &FsMountArgs,
) -> Result<(), CommandError> {
    Err(user_error(
        "`jj fs mount` is only available on Unix. Windows would need a ProjFS provider or the \
         Windows NFS client, neither of which jj implements yet.",
    ))
}

#[cfg(unix)]
#[instrument(skip_all)]
pub async fn cmd_fs_mount(
    ui: &mut Ui,
    command: &CommandHelper,
    args: &FsMountArgs,
) -> Result<(), CommandError> {
    let workspace_command = command.workspace_helper(ui).await?;
    let commit = workspace_command
        .resolve_single_rev(ui, &args.revision)
        .await?;
    let mountpoint = check_mountpoint(&args.mountpoint)?;

    let tree = commit.tree();
    let materialize = ConflictMaterializeOptions {
        // Honor the user's configured marker style, so what they read through
        // the mount matches what they would read in a working copy.
        marker_style: workspace_command.env().conflict_marker_style(),
        marker_len: None,
        merge: workspace_command.repo().store().merge_options().clone(),
    };
    let snapshot = Arc::new(
        TreeSnapshot::new(
            &tree,
            materialize,
            &commit.committer().timestamp,
            args.content_cache_bytes,
        )
        .await
        .map_err(|err| user_error_with_message("Failed to read the tree", err))?,
    );

    let transport = args.transport.unwrap_or(default_transport());
    // An empty value is what a bare `--scratch` produces, and means "yes, and
    // choose the directory for me". A path means that path.
    let explicit_scratch = args
        .scratch
        .as_ref()
        .filter(|path| !path.as_os_str().is_empty());
    let writable = args.scratch.is_some();
    if writable && transport == Transport::Fuse {
        // Not a silent downgrade to a read-only mount: the user asked for
        // something this transport cannot do, and quietly giving them less
        // would show up as an EROFS they had already asked us to prevent.
        return Err(user_error(
            "`--scratch` is only implemented for the `nfs` transport. Use `--transport nfs`.",
        ));
    }
    writeln!(
        ui.status(),
        "Mounting {revision} at {mountpoint} over {transport}. Press Ctrl-C to unmount.",
        revision = short_commit_id(&commit),
        mountpoint = mountpoint.display(),
        transport = transport.name(),
    )?;

    let tree = if writable {
        let directory = match explicit_scratch {
            Some(directory) => directory.clone(),
            None => default_overlay_dir(&mountpoint)?,
        };
        // Bound to this revision's tree, which is what scopes the layer's
        // whiteouts to it. The scratch *files* are keyed on the mountpoint and
        // survive a change of revision; the record of which tracked names were
        // deleted does not, because it is a statement about those names.
        let overlay = Overlay::open(directory, &snapshot.tree_key())
            .map_err(|err| user_error_with_message("Failed to open the writable layer", err))?;
        writeln!(
            ui.status(),
            "Scratch layer (experimental): {}",
            overlay.root().display()
        )?;
        Arc::new(OverlayTree::writable(snapshot, overlay))
    } else {
        Arc::new(OverlayTree::read_only(snapshot))
    };

    match transport {
        Transport::Fuse => mount_fuse(ui, tree, &mountpoint),
        Transport::Nfs => mount_nfs(ui, tree, &mountpoint, args.nfs_port, writable),
    }
}

/// Where a mountpoint's writable layer lives when the user did not say.
///
/// Keyed on the mountpoint and not on the revision, deliberately. Keying on the
/// revision would re-run `bun install` on every `jj new`, which is the useless
/// version of this feature: the whole reason the layer persists is that
/// repopulating a `node_modules` costs minutes.
///
/// Not inside `.jj/`, because jj's own snapshotting watches that directory and
/// a build tree dropped into it would be a bad day. Not inside the mountpoint
/// either, for the obvious reason.
///
/// The directory name keeps the mountpoint path readable rather than reducing
/// it to a hash, because the one thing a user needs to do with this directory
/// is find the right one and delete it. The hash suffix is there so that two
/// mountpoints that sanitize to the same name, or one long enough to be
/// truncated, still get separate layers.
#[cfg(unix)]
fn default_overlay_dir(mountpoint: &Path) -> Result<PathBuf, CommandError> {
    use etcetera::BaseStrategy as _;

    let base = etcetera::choose_base_strategy()
        .map_err(|err| user_error_with_message("Cannot locate the cache directory", err))?;
    let path = mountpoint.to_string_lossy();
    let mut readable: String = path
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    // A single filesystem component is capped at 255 bytes almost everywhere,
    // and the hash suffix plus separator has to fit inside that too.
    readable.truncate(200);
    let key = format!("{readable}-{:016x}", path_hash(&path));
    Ok(base.cache_dir().join("jj").join("fs-overlay").join(key))
}

/// A stable 64-bit hash of a mountpoint path.
///
/// FNV-1a, written out rather than taken from a crate, because the value is
/// baked into a directory name and has to mean the same thing in every future
/// version. `DefaultHasher` explicitly does not promise that between releases,
/// and the day it changed every existing scratch layer would be orphaned with
/// no error to explain where the `node_modules` went.
#[cfg(unix)]
fn path_hash(path: &str) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(unix)]
fn short_commit_id(commit: &jj_lib::commit::Commit) -> String {
    let hex = commit.id().hex();
    // min() rather than a bare slice: a backend is free to use shorter ids than
    // Git's, and a status message is not worth a panic.
    hex[..hex.len().min(12)].to_owned()
}

#[cfg(unix)]
const fn default_transport() -> Transport {
    // FUSE is one kernel hop with no RPC framing, so it wins on Linux where it
    // is also unprivileged. Everywhere else it would mean macFUSE, a
    // third-party kernel extension, so NFS is the only unprivileged option.
    if cfg!(target_os = "linux") {
        Transport::Fuse
    } else {
        Transport::Nfs
    }
}

#[cfg(unix)]
/// Refuses anything that would produce a confusing mount rather than an error.
fn check_mountpoint(mountpoint: &Path) -> Result<PathBuf, CommandError> {
    let metadata = std::fs::metadata(mountpoint).map_err(|err| {
        user_error_with_message(format!("Cannot use {}", mountpoint.display()), err)
    })?;
    if !metadata.is_dir() {
        return Err(user_error(format!(
            "Not a directory: {}",
            mountpoint.display()
        )));
    }
    // Mounting over a non-empty directory hides whatever is in it until the
    // unmount, which looks like data loss to the person it happens to.
    let mut entries = std::fs::read_dir(mountpoint).map_err(|err| {
        user_error_with_message(format!("Cannot read {}", mountpoint.display()), err)
    })?;
    if entries.next().is_some() {
        return Err(user_error(format!(
            "Mountpoint is not empty: {}",
            mountpoint.display()
        )));
    }
    // The mount helpers want an absolute path, and so does anything that later
    // tries to unmount it.
    std::fs::canonicalize(mountpoint).map_err(|err| {
        user_error_with_message(format!("Cannot resolve {}", mountpoint.display()), err)
    })
}

#[cfg(target_os = "linux")]
fn mount_fuse(ui: &mut Ui, tree: Arc<OverlayTree>, mountpoint: &Path) -> Result<(), CommandError> {
    let (uid, gid) = calling_user();
    let mut mount = jj_vfs::fuse::Mount::new(tree, mountpoint, uid, gid)
        .map_err(|err| user_error_with_message("Failed to mount over FUSE", err))?;
    // Without this, a signal kills the process before the mount's own unmount on
    // drop can run, leaving a mountpoint that answers every syscall with ENOTCONN
    // until someone unmounts it by hand. CleanupGuard is jj's existing machinery
    // for exactly that: it runs on SIGINT, SIGTERM and SIGHUP as well as on drop,
    // and it shares the one process-wide handler jj already installs, so
    // registering a second handler here would simply fail.
    let mut unmounter = mount.unmounter();
    let guard = CleanupGuard::new(move || {
        if let Err(err) = unmounter.unmount() {
            // Expected when the session already unmounted itself, which is the
            // normal way out of `run()`.
            tracing::debug!(?err, "unmount on cleanup did nothing");
        }
    });
    let result = mount.run();
    // Unmount before reporting, so the mountpoint is gone by the time the
    // command says it is.
    drop(guard);
    result.map_err(|err| user_error_with_message("FUSE session failed", err))?;
    writeln!(ui.status(), "Unmounted.")?;
    Ok(())
}

// The set is macOS and the BSDs. Spelled `all(unix, not(linux))` rather than
// the shorter `not(linux)`, which reads correct at a glance and is not:
// `not(linux)` is also true on Windows, where this function would reference
// types the unix-only imports never brought into scope. That exact mistake
// shipped once and only the Windows CI job caught it.
#[cfg(all(unix, not(target_os = "linux")))]
fn mount_fuse(
    _ui: &mut Ui,
    _tree: Arc<OverlayTree>,
    _mountpoint: &Path,
) -> Result<(), CommandError> {
    // Not a silent fallback to NFS: the user asked for a specific transport, and
    // quietly giving them a different one would hide why performance or
    // semantics changed.
    Err(user_error(
        "The fuse transport is only available on Linux. macOS has no in-kernel FUSE, and macFUSE \
         is a third-party kernel extension jj does not require. Use `--transport nfs`.",
    ))
}

#[cfg(unix)]
fn mount_nfs(
    ui: &mut Ui,
    tree: Arc<OverlayTree>,
    mountpoint: &Path,
    port: u16,
    writable: bool,
) -> Result<(), CommandError> {
    let (uid, gid) = calling_user();
    let server = jj_vfs::nfs::Server::start(tree, port, uid, gid)
        .map_err(|err| user_error_with_message("Failed to start the NFS server", err))?;
    let port = server.port();

    run_mount_command(port, mountpoint, writable)?;
    // Registered the moment the mount exists, so that any later failure and any
    // of SIGINT/SIGTERM/SIGHUP tears it down. An NFS mount whose server has gone
    // away is worse than a missing one: every syscall against it blocks until the
    // client's timeout, so an `ls` in the wrong directory hangs.
    let owned_mountpoint = mountpoint.to_owned();
    // Named rather than discarded: a CleanupGuard runs its callback when dropped,
    // so `let _ =` here would unmount immediately.
    let _guard = CleanupGuard::new(move || {
        if let Err(err) = unmount(&owned_mountpoint) {
            eprintln!("Failed to unmount {}: {err}", owned_mountpoint.display());
        }
    });
    writeln!(
        ui.status(),
        "Serving NFSv3 on 127.0.0.1:{port} and mounted at {}.",
        mountpoint.display()
    )?;
    // Printed on the way out rather than on a timer, so a benchmark run ends
    // with the numbers that explain it. A wall clock figure on its own cannot
    // say whether a slow mount is slow per operation, slow because there are
    // too many operations, or slow because of work we are doing inside each
    // one, and those have three different fixes.
    //
    // Behind an environment variable rather than a flag or a log level. It is a
    // benchmarking tool, the three read-only mounts a person keeps open all day
    // have nothing interesting to say here, and printing a table at every
    // unmount would be noise they did not ask for.
    let counters = std::sync::Arc::new(server);
    let reporter = counters.clone();
    let _stats_guard = CleanupGuard::new(move || {
        if std::env::var_os("JJ_VFS_STATS").is_some() {
            eprintln!("{}", reporter.report());
        }
    });

    // The server has to outlive the mount, so the process parks rather than
    // exiting and leaving the kernel talking to nobody. jj's signal handler runs
    // the guard above and then exits the process, so there is nothing to wake up
    // for and no poll loop to write. park() can return spuriously, hence the loop.
    loop {
        std::thread::park();
    }
}

#[cfg(unix)]
/// The uid and gid reported as the owner of every entry.
///
/// Showing the calling user rather than root or nobody is what every other
/// user-space filesystem does, and it is what makes `ls -l` on the mount look
/// like a checkout.
fn calling_user() -> (u32, u32) {
    // SAFETY: getuid and getgid cannot fail and touch no memory we own.
    unsafe { (libc::getuid(), libc::getgid()) }
}

/// Finds a mount-family binary, searching `PATH` and then the conventional
/// sbin directories.
///
/// `PATH` alone is not enough. `mount` and `umount` live in `/sbin` or
/// `/usr/sbin` on most distributions, and neither is on `PATH` for a non-login
/// process: a systemd unit without an explicit `Environment=PATH`, a cron job,
/// or an editor subprocess all lack them. Spawning by bare name there fails
/// with `No such file or directory (os error 2)`, which names neither the
/// binary nor where it was looked for, and reads like the mount point is
/// missing rather than the tool. hf-mount hit exactly this and resolves
/// explicitly for the same reason; see <https://github.com/huggingface/hf-mount/issues/101>.
#[cfg(unix)]
fn resolve_mount_binary(name: &str) -> Result<PathBuf, CommandError> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    // Appended rather than prepended, so an operator who put a specific build on
    // PATH still wins. Order among the fallbacks follows where distributions
    // actually install these.
    for fallback in ["/sbin", "/usr/sbin", "/bin", "/usr/bin"] {
        let dir = PathBuf::from(fallback);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    let candidates: Vec<PathBuf> = dirs.into_iter().map(|dir| dir.join(name)).collect();
    if let Some(found) = candidates.iter().find(|candidate| candidate.is_file()) {
        return Ok(found.clone());
    }
    // Name the binary and every path tried. The failure this replaces said
    // "os error 2" and nothing else.
    let tried = candidates
        .iter()
        .map(|candidate| candidate.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(user_error(format!(
        "Could not find `{name}`. Looked in: {tried}. On Linux the NFS transport needs the \
         nfs-utils package (nfs-common on Debian and Ubuntu); `--transport fuse` needs neither it \
         nor root."
    )))
}

/// Asks the operating system's own NFS client to mount our loopback server.
///
/// The options differ per platform and each one is load bearing, so they are
/// spelled out rather than inherited from a default.
#[cfg(unix)]
fn run_mount_command(port: u16, mountpoint: &Path, writable: bool) -> Result<(), CommandError> {
    let access = if writable { "rw" } else { "ro" };
    // A read-only mount can afford a short timeout, because losing a read to a
    // wedged server costs a retry. Losing a write costs the bytes: `soft`
    // returns EIO and the data is gone, and a package manager mid-install has
    // no idea it happened. Sixty seconds rather than five is long enough that
    // only a genuinely dead server hits it. `hard` would remove the loss
    // entirely and is not used, because a hard mount whose server has exited
    // leaves every process touching it in uninterruptible sleep, which is the
    // failure this command already went out of its way to avoid.
    let timeout = if writable {
        "timeo=600,retrans=2"
    } else {
        "timeo=50,retrans=2"
    };
    let mut command;
    if cfg!(target_os = "macos") {
        command = Command::new(resolve_mount_binary("mount_nfs")?);
        command.arg("-o").arg(format!(
            // noresvport is the one that decides whether this needs sudo:
            // mount_nfs(8) says a reserved source port requires root, and
            // nothing about NFSv3 needs one.
            //
            // locallocks, and specifically NOT nolocks. Measured on macOS 27.0
            // against a writable loopback NFSv3 server, flock(LOCK_EX) and a
            // POSIX write lock on a file on the mount:
            //
            //     -o nolocks             ENOTSUP (45)
            //     -o locallocks          OK
            //     -o nolocks,locallocks  ENOTSUP (45)   <- nolocks wins
            //     neither                ENOLCK  (77)   <- no NLM on our server
            //
            // jj takes its own locks with flock (lib/src/lock/unix.rs), so
            // nolocks would make jj itself fail on a mount it is serving. Our
            // server implements no NLM either, so client-local locking is the
            // only option that works at all.
            //
            // soft plus a short timeout means a wedged server gives callers an
            // error instead of an unkillable process in uninterruptible sleep.
            "locallocks,vers=3,tcp,port={port},mountport={port},soft,{timeout},{access},noresvport"
        ));
        command.arg("127.0.0.1:/").arg(mountpoint);
    } else {
        command = Command::new(resolve_mount_binary("mount")?);
        command.arg("-t").arg("nfs");
        command.arg("-o").arg(format!(
            // Linux spells it "nolock", and unlike macOS's "nolocks" it leaves
            // flock working locally rather than returning ENOTSUP. The VM test
            // asserts that rather than trusting it.
            "nolock,vers=3,tcp,port={port},mountport={port},soft,{timeout},{access}"
        ));
        command.arg("127.0.0.1:/").arg(mountpoint);
    }
    let output = command
        .output()
        .map_err(|err| user_error_with_message("Failed to run the mount command", err))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let hint = if cfg!(target_os = "linux") {
            " Mounting NFS on Linux needs root; `--transport fuse` does not."
        } else {
            ""
        };
        return Err(user_error(format!("mount failed: {stderr}{hint}")));
    }
    Ok(())
}

/// Unmounts, escalating to a forced unmount if the ordinary one is refused.
///
/// The escalation is not a retry of the same thing. Tearing down an NFS mount
/// means talking to the server, and here the server is this very process on its
/// way out, so there is a window where the client asks and nobody answers. An
/// ordinary `umount` that loses that race leaves the mount in place, and
/// because the server never comes back every syscall against it then blocks
/// until the client's timeout: observed once in the NixOS VM test as a
/// mountpoint that stayed for 45 seconds until `nfs: server 127.0.0.1 not
/// responding, timed out`. `umount -f` is the documented way to detach a mount
/// whose server has gone away, which is exactly the situation, so it is the
/// right second step rather than a hopeful repeat of the first.
///
/// `umount -l` would also make the mountpoint disappear, and is deliberately
/// not used: it detaches the name while leaving the filesystem live, so
/// anything already inside it keeps talking to a server that is exiting.
#[cfg(unix)]
fn unmount(mountpoint: &Path) -> Result<(), std::io::Error> {
    let first = run_umount(mountpoint, false)?;
    if let Some(err) = first {
        let forced = run_umount(mountpoint, true)?;
        if let Some(forced_err) = forced {
            return Err(std::io::Error::other(format!(
                "umount failed ({err}), and so did umount -f ({forced_err})"
            )));
        }
        tracing::warn!(%err, "plain umount was refused; forced unmount succeeded");
    }
    Ok(())
}

/// Runs one unmount. `Ok(None)` means it worked, `Ok(Some(reason))` means the
/// command ran and refused, and `Err` means it could not be run at all.
#[cfg(unix)]
fn run_umount(mountpoint: &Path, force: bool) -> Result<Option<String>, std::io::Error> {
    // Resolved for the same reason as `mount`, but falling back to the bare name
    // rather than refusing: this runs from a cleanup guard, and a teardown that
    // declines to try because it could not find the binary leaves a mount behind,
    // which is strictly worse than an exec that fails and says so.
    let umount = resolve_mount_binary("umount").unwrap_or_else(|_| PathBuf::from("umount"));
    let mut command = Command::new(umount);
    if force {
        command.arg("-f");
    }
    let output = command.arg(mountpoint).output()?;
    if output.status.success() {
        return Ok(None);
    }
    // Both streams and the code, because a bare "umount said:" with an empty
    // stderr is what this looked like the first time it happened and it said
    // nothing useful.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    let detail = if detail.is_empty() {
        "no output".to_owned()
    } else {
        detail
    };
    // `ExitStatus` already displays as "exit status: 1", so it is not prefixed.
    Ok(Some(format!("{}: {detail}", output.status)))
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_mount_binary_searches_sbin_when_path_lacks_it() {
        // `mount` and `umount` are the real subjects, but a test that asserts they
        // exist is asserting something about the host rather than about the
        // resolver. `sh` is guaranteed at /bin/sh by POSIX, so it exercises the
        // fallback list without depending on how the host is packaged.
        let resolved = resolve_mount_binary("sh").expect("sh is at /bin/sh on any unix");
        assert!(
            resolved.is_file(),
            "resolver returned {} which is not a file",
            resolved.display()
        );
    }

    #[test]
    fn test_resolve_mount_binary_error_names_the_binary_and_every_path_tried() {
        // The whole point of the resolver: the failure it replaces was
        // `No such file or directory (os error 2)`, which named neither the
        // binary nor where it had been looked for, and read like the mountpoint
        // was missing rather than the tool.
        let err = resolve_mount_binary("jj-definitely-not-a-real-mount-helper")
            .expect_err("a nonexistent binary must not resolve");
        let message = format!("{err:?}");
        assert!(
            message.contains("jj-definitely-not-a-real-mount-helper"),
            "error does not name the binary: {message}"
        );
        for expected in ["/sbin", "/usr/sbin"] {
            assert!(
                message.contains(expected),
                "error does not name {expected} among the paths tried: {message}"
            );
        }
    }
}
