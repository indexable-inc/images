//! Raw host block devices as VM disks (macOS, Virtualization.framework).
//!
//! Every vmkit disk used to be an image file on APFS, which costs sparse-file
//! fragmentation, COW metadata churn, and host-side double caching for
//! persistent high-churn guests. macOS 14+ can attach a real disk node
//! (`/dev/disk4`, `/dev/rdisk4s2`) straight to virtio-blk via
//! `VZDiskBlockDeviceStorageDeviceAttachment`, bypassing APFS entirely.
//!
//! [`storage_attachment`] is the one seam the guest builders use: a path that
//! stats as a regular file becomes a `VZDiskImageStorageDeviceAttachment`
//! (unchanged behavior); one that stats as a block/char device becomes the
//! block-device attachment. Detection is by stat, never by string prefix, so
//! an image named `dev-disk.raw` and a symlink to a device node (e.g. a mac
//! bundle's `disk.img` pointing at a partition) both do the right thing.
//!
//! Safety and privilege model (prior art: vfkit `type=dev`, lima-vm/lima#4866,
//! its security revert #5113, and the hardened redo #5117):
//!
//! - A device that currently hosts mounted filesystems is refused unless the
//!   caller passes `--force`: guest and host writing the same filesystem
//!   corrupts it irreversibly. The check walks `diskutil list -plist`
//!   including APFS physical-store linkage: an APFS container carved onto
//!   `disk0s2` mounts its volumes under a synthesized `disk3`, so a naive
//!   mount-table scan of `disk0` sees nothing while `/` lives on it.
//! - `/dev/disk*` is root-owned, and running the VM as root is not an option.
//!   Following Apple's guidance (and lima's fd-handoff helper), when the
//!   direct open fails with permission denied, vmkit re-runs itself under
//!   plain **interactive** `sudo` as a tiny hidden `open-block-device`
//!   subcommand that opens the node and passes the fd back over a private
//!   unix socket (`SCM_RIGHTS`); the VM process stays unprivileged. No
//!   sudoers entry is ever installed: lima's `NOPASSWD` helper entry was a
//!   root file-open oracle for every local user (lima-vm/lima#5112) and got
//!   the whole feature reverted. Interactive sudo keeps a human consenting to
//!   exactly the command line they can read.
//! - Both sides distrust the channel anyway: the helper only opens literal
//!   `/dev/[r]diskN[sM...]` nodes (never symlinks, `O_NOFOLLOW`) and only
//!   talks to a socket in a `0700` directory owned by the sudo-invoking user
//!   (`SUDO_UID`); the parent accepts only a root peer (`getpeereid`) and
//!   revalidates the received fd (`fstat` device type and `st_rdev` against
//!   the requested node) before Virtualization.framework sees it.
//!
//! The libkrun backend deliberately gets none of this: it opens disks by path
//! as plain files, and no fd route exists there (macOS returns `ENOTTY` for
//! `F_SETFL` on disk fds, and reopening `/dev/fd/N` re-checks vnode
//! permissions, so a root-opened fd does not help an unprivileged process;
//! see lima-vm/lima#5104). `boot-linux` rejects device paths loudly in
//! [`crate::linuxkrun`].

use std::fs::File;
use std::io::ErrorKind;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2_foundation::NSFileHandle;
use objc2_virtualization::{
    VZDiskBlockDeviceStorageDeviceAttachment, VZDiskImageStorageDeviceAttachment,
    VZDiskSynchronizationMode, VZStorageDeviceAttachment,
};
use serde::Deserialize;
use snafu::{ResultExt, Snafu};

use crate::imp::{file_url, ns_error_message};

/// Environment variable carrying the pre-re-exec binary path (typically the
/// immutable `/nix/store` one). The sudo helper prefers it over
/// `current_exe()`, which after the entitlement re-exec points at the
/// user-writable signed cache copy; the path the sudo prompt shows should be
/// the one a user can trust not to change underneath them.
pub const ORIG_EXE_ENV: &str = "IX_VMKIT_ORIG_EXE";

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("cannot stat disk {path:?}: {source}"))]
    Stat {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display(
        "disk {path:?} is neither a regular image file nor a block device; \
         a VM disk must be one of the two"
    ))]
    UnsupportedDiskType { path: PathBuf },
    #[snafu(display(
        "{path:?} is not a macOS disk device node; only literal /dev/diskN[sM...] \
         or /dev/rdiskN[sM...] paths can attach as raw block devices"
    ))]
    NotDiskNode { path: PathBuf },
    #[snafu(display("scanning mounted filesystems for {path:?} failed: {source}"))]
    MountScan {
        path: PathBuf,
        source: crate::provision::Error,
    },
    #[snafu(display("could not parse `diskutil list -plist` output: {source}"))]
    ParseDiskList { source: serde_json::Error },
    #[snafu(display(
        "device {path:?} hosts mounted filesystems ({mounts}); unmount them first \
         (`diskutil unmountDisk {path:?}`) or pass --force to attach anyway \
         (DANGEROUS: the guest and the host writing concurrently corrupts them irreversibly)"
    ))]
    Mounted { path: PathBuf, mounts: String },
    #[snafu(display("opening block device {path:?} failed: {source}"))]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("setting up the fd-handoff socket failed: {source}"))]
    Handoff { source: std::io::Error },
    #[snafu(display("spawning sudo for the root open-block-device helper failed: {source}"))]
    SudoSpawn { source: std::io::Error },
    #[snafu(display(
        "the sudo root helper exited ({status}) without delivering a file descriptor \
         for {path:?}; its error (or sudo's) is printed above"
    ))]
    HelperFailed { path: PathBuf, status: String },
    #[snafu(display("fd handoff for {path:?} failed: {message}"))]
    Protocol { path: PathBuf, message: String },
    #[snafu(display(
        "refusing the fd-handoff connection: peer euid {euid} is not root \
         (something other than the sudo helper connected to the socket)"
    ))]
    PeerNotRoot { euid: u32 },
    #[snafu(display(
        "the opened descriptor does not match {path:?} (device type or number differ); \
         refusing to hand it to the VM"
    ))]
    DeviceMismatch { path: PathBuf },
    #[snafu(display(
        "open-block-device must run as root; vmkit invokes it via sudo itself when a \
         raw disk needs opening (it is not for direct use)"
    ))]
    HelperNotRoot,
    #[snafu(display(
        "open-block-device requires the sudo invocation context (SUDO_UID) to verify \
         the handoff socket belongs to the invoking user"
    ))]
    NoSudoContext,
    #[snafu(display("refusing fd-handoff socket {path:?}: {message}"))]
    SocketInsecure { path: PathBuf, message: String },
    #[snafu(display("connecting to the fd-handoff socket {path:?} failed: {source}"))]
    Connect {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("sending the opened descriptor back failed: {source}"))]
    SendFd { source: std::io::Error },
    #[snafu(display("attaching disk image {path:?} failed: {message}"))]
    AttachImage { path: PathBuf, message: String },
    #[snafu(display(
        "attaching block device {path:?} failed: {message} \
         (VZDiskBlockDeviceStorageDeviceAttachment needs macOS 14+)"
    ))]
    AttachDevice { path: PathBuf, message: String },
}

/// How a raw block-device disk synchronizes guest flushes with the host disk.
/// Explicit because the `F_FULLFSYNC` a `full` flush costs dominates guest
/// write latency regardless of the backing (image file or device).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum DiskSync {
    /// Forward every guest flush/barrier to the disk (safe; survives host
    /// panic and power loss at `F_FULLFSYNC` cost).
    #[default]
    Full,
    /// Never synchronize with the underlying storage (fast; the on-disk state
    /// after any host failure is undefined -- only for disposable data).
    None,
}

impl DiskSync {
    const fn to_vz(self) -> VZDiskSynchronizationMode {
        match self {
            Self::Full => VZDiskSynchronizationMode::Full,
            Self::None => VZDiskSynchronizationMode::None,
        }
    }
}

/// Disk-attachment policy flags, shared verbatim by every subcommand that can
/// attach a raw block device. Both fields only affect block devices; image
/// files ignore them.
#[derive(Clone, Copy, Debug, Default, clap::Args)]
pub struct DiskFlags {
    /// Synchronization mode for raw block-device disks.
    #[arg(long, value_enum, default_value_t = DiskSync::Full)]
    pub disk_sync: DiskSync,
    /// Attach a raw block device even while it hosts mounted filesystems.
    /// DANGEROUS: the guest and the host writing the same filesystem corrupts
    /// it irreversibly.
    #[arg(long)]
    pub force: bool,
}

/// What a disk path stats as (never string-sniffed; see the module docs).
enum DiskKind {
    Image,
    BlockDevice,
}

fn disk_kind(path: &Path) -> Result<DiskKind, Error> {
    // `metadata` follows symlinks: a bundle's `disk.img` symlinked at a device
    // node classifies as the device it points to.
    let meta = std::fs::metadata(path).context(StatSnafu { path })?;
    let file_type = meta.file_type();
    if file_type.is_block_device() || file_type.is_char_device() {
        Ok(DiskKind::BlockDevice)
    } else if file_type.is_file() {
        Ok(DiskKind::Image)
    } else {
        UnsupportedDiskTypeSnafu { path }.fail()
    }
}

/// Whether `path` currently stats as a device node. Lenient (a missing path is
/// just `false`): used for CLI decisions like the `--efi-vars` default, where
/// the authoritative error comes later from [`storage_attachment`].
pub fn is_device_node(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| {
        let file_type = meta.file_type();
        file_type.is_block_device() || file_type.is_char_device()
    })
}

/// Build the VZ storage attachment for `path`: a disk image file or a raw
/// block device, by stat. The returned attachment is what
/// `VZVirtioBlockDeviceConfiguration` wraps; disks always attach read-write
/// (vmkit's guests own their disks while running).
pub fn storage_attachment(
    path: &Path,
    flags: DiskFlags,
) -> Result<Retained<VZStorageDeviceAttachment>, Error> {
    match disk_kind(path)? {
        DiskKind::Image => {
            let url = file_url(path);
            let attach = unsafe {
                VZDiskImageStorageDeviceAttachment::initWithURL_readOnly_error(
                    VZDiskImageStorageDeviceAttachment::alloc(),
                    &url,
                    false,
                )
            }
            .map_err(|error| Error::AttachImage {
                path: path.to_owned(),
                message: ns_error_message(&error),
            })?;
            Ok(attach.into_super())
        }
        DiskKind::BlockDevice => {
            let file = open_for_vm(path, flags.force)?;
            // The handle takes ownership of the fd (`closeOnDealloc`) and the
            // attachment retains the handle, which must stay open until the VM
            // starts -- exactly the lifetime the retain chain provides.
            let handle = NSFileHandle::initWithFileDescriptor_closeOnDealloc(
                NSFileHandle::alloc(),
                file.into_raw_fd(),
                true,
            );
            let attach = unsafe {
                VZDiskBlockDeviceStorageDeviceAttachment::initWithFileHandle_readOnly_synchronizationMode_error(
                    VZDiskBlockDeviceStorageDeviceAttachment::alloc(),
                    &handle,
                    false,
                    flags.disk_sync.to_vz(),
                )
            }
            .map_err(|error| Error::AttachDevice {
                path: path.to_owned(),
                message: ns_error_message(&error),
            })?;
            Ok(attach.into_super())
        }
    }
}

/// Open a raw disk device for the VM: resolve the path to a literal `/dev`
/// node, refuse a device hosting mounted filesystems (unless forced), open it
/// directly when permissions allow, otherwise through the interactive-sudo
/// helper, and revalidate whatever descriptor came back.
fn open_for_vm(path: &Path, force: bool) -> Result<File, Error> {
    // Resolve symlinks up front (e.g. a bundle's `disk.img` -> /dev/disk5), so
    // every later check and the sudo prompt see the literal device node.
    let resolved = std::fs::canonicalize(path).context(StatSnafu { path })?;
    let id = device_id(&resolved)?;
    let meta = std::fs::symlink_metadata(&resolved).context(StatSnafu { path: &resolved })?;
    if force {
        eprintln!(
            "vmkit: --force: skipping the mounted-filesystem check for {}",
            resolved.display()
        );
    } else {
        let mounts = mounted_filesystems(&id, &resolved)?;
        if !mounts.is_empty() {
            return MountedSnafu {
                path: resolved,
                mounts: mounts.join("; "),
            }
            .fail();
        }
    }
    let file = match open_direct(&resolved) {
        Ok(file) => file,
        // Permission denied (root-owned /dev node): Apple's recommended split
        // -- open in a root process, pass the fd, keep the VM unprivileged.
        Err(source) if source.kind() == ErrorKind::PermissionDenied => open_via_sudo(&resolved)?,
        Err(source) => {
            return Err(Error::Open {
                path: resolved,
                source,
            });
        }
    };
    // Revalidate whatever we got (own open or helper handoff) before
    // Virtualization.framework sees it: still a device, still *this* device.
    let opened = file.metadata().context(StatSnafu { path: &resolved })?;
    let opened_type = opened.file_type();
    if !(opened_type.is_block_device() || opened_type.is_char_device())
        || opened.rdev() != meta.rdev()
    {
        return DeviceMismatchSnafu { path: resolved }.fail();
    }
    Ok(file)
}

/// Open the device node read-write. `O_NOFOLLOW` is belt-and-braces with the
/// canonicalized path (devfs cannot hold user-planted symlinks, but the check
/// is free).
fn open_direct(path: &Path) -> std::io::Result<File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

/// Open a root-owned device via a sudo'd re-exec of vmkit's hidden
/// `open-block-device` subcommand, receiving the fd over a private unix
/// socket. Interactive: sudo may prompt on the controlling terminal (stdio is
/// inherited); there is deliberately no sudoers automation (see module docs).
fn open_via_sudo(path: &Path) -> Result<File, Error> {
    eprintln!(
        "vmkit: {} is not accessible by this user; opening it as root via sudo \
         (a password prompt may follow)",
        path.display()
    );
    // Explicitly 0700: tempfile creates directories with the process umask
    // (0755 in practice), and the helper refuses any socket directory that is
    // group/other accessible -- the run that discovered this failed exactly
    // there.
    let dir = tempfile::Builder::new()
        .prefix("vmkit-fd-")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .context(HandoffSnafu)?;
    let socket = dir.path().join("fd.sock");
    let listener = UnixListener::bind(&socket).context(HandoffSnafu)?;
    listener.set_nonblocking(true).context(HandoffSnafu)?;
    let helper = std::env::var_os(ORIG_EXE_ENV).map_or_else(
        || std::env::current_exe().context(HandoffSnafu),
        |exe| Ok(PathBuf::from(exe)),
    )?;
    let mut child = Command::new("/usr/bin/sudo")
        .arg("--")
        .arg(&helper)
        .arg("open-block-device")
        .arg("--device")
        .arg(path)
        .arg("--socket")
        .arg(&socket)
        .spawn()
        .context(SudoSpawnSnafu)?;
    let stream = wait_for_helper(&listener, &mut child, path)?;
    let euid = peer_euid(&stream).map_err(|error| Error::Protocol {
        path: path.to_owned(),
        message: format!("getpeereid: {error}"),
    })?;
    if euid != 0 {
        let _ = child.wait();
        return PeerNotRootSnafu { euid }.fail();
    }
    let fd = recv_fd(&stream).map_err(|error| Error::Protocol {
        path: path.to_owned(),
        message: error.to_string(),
    })?;
    // Reap the helper; it exits right after sending.
    let _ = child.wait();
    Ok(File::from(fd))
}

/// Wait for the root helper to connect. There is no overall deadline while
/// sudo is still running: an interactive password prompt takes as long as the
/// human takes. A helper that exits without connecting (wrong password,
/// validation failure) fails here immediately -- almost: one short grace
/// window covers the helper connecting and exiting before our first accept
/// (the queued connection is still deliverable).
fn wait_for_helper(
    listener: &UnixListener,
    child: &mut std::process::Child,
    path: &Path,
) -> Result<UnixStream, Error> {
    let mut exited: Option<String> = None;
    let mut grace: u32 = 0;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(source) => return Err(Error::Handoff { source }),
        }
        if exited.is_none() {
            match child.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => exited = Some(status.to_string()),
                Err(error) => {
                    return ProtocolSnafu {
                        path,
                        message: format!("waiting for the sudo helper: {error}"),
                    }
                    .fail();
                }
            }
        }
        if let Some(status) = &exited {
            grace += 1;
            if grace > 4 {
                return HelperFailedSnafu {
                    path,
                    status: status.clone(),
                }
                .fail();
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Serve one `open-block-device` request: the root side of the fd handoff.
/// Validates everything it can (see module docs) before opening and sending.
pub fn serve_open_block_device(device: &Path, socket: &Path) -> Result<(), Error> {
    // Safety: geteuid cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        return HelperNotRootSnafu.fail();
    }
    let sudo_uid = sudo_uid().ok_or(Error::NoSudoContext)?;
    // Only literal disk nodes: never an arbitrary root-readable file, never a
    // symlink (lima#5117's request hardening).
    device_id(device)?;
    let meta = std::fs::symlink_metadata(device).context(StatSnafu { path: device })?;
    let file_type = meta.file_type();
    if !(file_type.is_block_device() || file_type.is_char_device()) {
        return NotDiskNodeSnafu { path: device }.fail();
    }
    check_socket_private(socket, sudo_uid)?;
    let file = open_direct(device).context(OpenSnafu { path: device })?;
    // Revalidate post-open: still the very device the request named.
    let opened = file.metadata().context(StatSnafu { path: device })?;
    let opened_type = opened.file_type();
    if !(opened_type.is_block_device() || opened_type.is_char_device())
        || opened.rdev() != meta.rdev()
    {
        return DeviceMismatchSnafu { path: device }.fail();
    }
    let stream = UnixStream::connect(socket).context(ConnectSnafu { path: socket })?;
    send_fd(&stream, file.as_fd()).context(SendFdSnafu)?;
    Ok(())
}

/// The uid sudo says invoked it. Root-without-sudo is refused ([`Error::NoSudoContext`]):
/// the socket-ownership check below has no meaning without it.
fn sudo_uid() -> Option<u32> {
    std::env::var("SUDO_UID").ok()?.parse().ok()
}

/// The helper only talks to a private handoff socket: parent directory `0700`
/// and owned by the sudo-invoking user, socket likewise owned by that user.
/// Anything looser could let another local user receive the root-opened fd
/// (lima#5117's socket hardening).
fn check_socket_private(socket: &Path, sudo_uid: u32) -> Result<(), Error> {
    let insecure = |message: String| Error::SocketInsecure {
        path: socket.to_owned(),
        message,
    };
    let dir = socket
        .parent()
        .ok_or_else(|| insecure("socket has no parent directory".to_owned()))?;
    let dir_meta = std::fs::symlink_metadata(dir).context(StatSnafu { path: dir })?;
    if !dir_meta.is_dir() {
        return Err(insecure("socket parent is not a directory".to_owned()));
    }
    if dir_meta.uid() != sudo_uid {
        return Err(insecure(format!(
            "socket directory owner uid {} is not the sudo-invoking uid {sudo_uid}",
            dir_meta.uid()
        )));
    }
    if dir_meta.mode() & 0o077 != 0 {
        return Err(insecure(format!(
            "socket directory mode {:o} is group/other accessible (need 0700)",
            dir_meta.mode() & 0o777
        )));
    }
    let sock_meta = std::fs::symlink_metadata(socket).context(StatSnafu { path: socket })?;
    if !sock_meta.file_type().is_socket() {
        return Err(insecure("path is not a unix socket".to_owned()));
    }
    if sock_meta.uid() != sudo_uid {
        return Err(insecure(format!(
            "socket owner uid {} is not the sudo-invoking uid {sudo_uid}",
            sock_meta.uid()
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Device-path grammar.

/// The normalized BSD identifier (`disk4s2`) for a literal macOS disk node
/// path. Both the buffered (`/dev/diskN`) and raw (`/dev/rdiskN`) nodes are
/// accepted -- they name the same disk -- and only literal `diskN[sM...]`
/// shapes pass: `/dev/null`, `/dev/fd/3`, nested paths, relative paths, and
/// anything outside `/dev` are rejected, so the root helper can never be
/// pointed at an arbitrary root-readable file.
fn device_id(path: &Path) -> Result<String, Error> {
    let id = path
        .to_str()
        .and_then(|p| p.strip_prefix("/dev/"))
        .map(|node| node.strip_prefix('r').unwrap_or(node))
        .filter(|id| valid_bsd_id(id));
    id.map_or_else(|| NotDiskNodeSnafu { path }.fail(), |id| Ok(id.to_owned()))
}

/// `disk<digits>` followed by zero or more `s<digits>` groups, nothing else.
fn valid_bsd_id(id: &str) -> bool {
    let Some(mut rest) = id.strip_prefix("disk") else {
        return false;
    };
    loop {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        rest = &rest[digits..];
        if rest.is_empty() {
            return true;
        }
        match rest.strip_prefix('s') {
            Some(after) => rest = after,
            None => return false,
        }
    }
}

// ---------------------------------------------------------------------------
// Mounted-filesystem refusal.

/// `diskutil list -plist` result: every disk with its partitions, APFS
/// volumes, and (for synthesized APFS containers) the physical stores they
/// live on.
#[derive(Deserialize)]
struct DiskList {
    #[serde(rename = "AllDisksAndPartitions")]
    disks: Vec<DiskEntry>,
}

#[derive(Deserialize)]
struct DiskEntry {
    #[serde(rename = "DeviceIdentifier")]
    device_identifier: String,
    /// Present when a filesystem sits directly on the whole disk.
    #[serde(rename = "MountPoint")]
    mount_point: Option<String>,
    #[serde(rename = "Partitions", default)]
    partitions: Vec<VolumeEntry>,
    #[serde(rename = "APFSVolumes", default)]
    apfs_volumes: Vec<VolumeEntry>,
    /// For a synthesized APFS container: the real partitions it lives on.
    #[serde(rename = "APFSPhysicalStores", default)]
    apfs_physical_stores: Vec<StoreRef>,
}

#[derive(Deserialize)]
struct VolumeEntry {
    #[serde(rename = "DeviceIdentifier")]
    device_identifier: String,
    #[serde(rename = "MountPoint")]
    mount_point: Option<String>,
    /// APFS snapshots mounted off this volume (e.g. the sealed system
    /// snapshot that is `/`).
    #[serde(rename = "MountedSnapshots", default)]
    mounted_snapshots: Vec<SnapshotRef>,
}

#[derive(Deserialize)]
struct SnapshotRef {
    #[serde(rename = "SnapshotBSD")]
    snapshot_bsd: Option<String>,
    #[serde(rename = "SnapshotMountPoint")]
    snapshot_mount_point: Option<String>,
}

#[derive(Deserialize)]
struct StoreRef {
    #[serde(rename = "DeviceIdentifier")]
    device_identifier: String,
}

/// Every mounted filesystem hosted by device `id` (directly, as a slice, or
/// through APFS physical-store linkage), as human-readable `dev at /mount`
/// strings. Non-empty means attaching `id` read-write is destructive.
fn mounted_filesystems(id: &str, path: &Path) -> Result<Vec<String>, Error> {
    let json = crate::provision::run_plist_json("diskutil", &["list", "-plist"])
        .context(MountScanSnafu { path })?;
    let list: DiskList = serde_json::from_str(&json).context(ParseDiskListSnafu)?;
    Ok(mounted_in(&list, id))
}

/// Pure matching core of [`mounted_filesystems`], separated for tests.
fn mounted_in(list: &DiskList, target: &str) -> Vec<String> {
    // Roots: the target plus, transitively, every synthesized disk whose APFS
    // physical store lives under an existing root. An APFS container carved
    // onto disk0s2 surfaces as its own whole-disk (say disk3) and *its*
    // volumes are what mount, so writing disk0 corrupts them even though no
    // mount entry names disk0.
    let mut roots = vec![target.to_owned()];
    loop {
        let mut grew = false;
        for disk in &list.disks {
            if disk
                .apfs_physical_stores
                .iter()
                .any(|store| under_any(&roots, &store.device_identifier))
                && !roots.contains(&disk.device_identifier)
            {
                roots.push(disk.device_identifier.clone());
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }

    let mut mounts = Vec::new();
    for disk in &list.disks {
        push_mount(
            &mut mounts,
            &roots,
            &disk.device_identifier,
            disk.mount_point.as_deref(),
        );
        for volume in disk.partitions.iter().chain(&disk.apfs_volumes) {
            push_mount(
                &mut mounts,
                &roots,
                &volume.device_identifier,
                volume.mount_point.as_deref(),
            );
            for snapshot in &volume.mounted_snapshots {
                if let Some(mount_point) = snapshot.snapshot_mount_point.as_deref() {
                    let snap_id = snapshot
                        .snapshot_bsd
                        .as_deref()
                        .unwrap_or(&volume.device_identifier);
                    if under_any(&roots, snap_id) || under_any(&roots, &volume.device_identifier) {
                        mounts.push(format!("{snap_id} (snapshot) at {mount_point}"));
                    }
                }
            }
        }
    }
    mounts
}

fn push_mount(mounts: &mut Vec<String>, roots: &[String], id: &str, mount_point: Option<&str>) {
    if let Some(mount_point) = mount_point
        && under_any(roots, id)
    {
        mounts.push(format!("{id} at {mount_point}"));
    }
}

/// Whether `id` equals one of `roots` or is a slice of one (`disk4` covers
/// `disk4s2` and `disk4s1s1`, never `disk40`).
fn under_any(roots: &[String], id: &str) -> bool {
    roots.iter().any(|root| {
        id == root
            || id
                .strip_prefix(root.as_str())
                .is_some_and(|rest| rest.starts_with('s'))
    })
}

// ---------------------------------------------------------------------------
// SCM_RIGHTS fd handoff. std has no stable ancillary-data API, so this is the
// classic sendmsg/recvmsg cmsg dance via libc, kept minimal: exactly one fd,
// one payload byte (control data cannot ride an empty message).

const HANDOFF_BYTE: u8 = 0x5A;
/// `size_of::<RawFd>()`, as the `c_uint` the `CMSG_*` macros take.
const FD_LEN: libc::c_uint = 4;
const _: () = assert!(std::mem::size_of::<RawFd>() == FD_LEN as usize);
/// Control buffer sized for one fd: `CMSG_SPACE(4)` is 16 on macOS; `u32`
/// storage guarantees `cmsghdr` alignment; 32 bytes leave headroom.
const CTRL_WORDS: usize = 8;

fn send_fd(stream: &UnixStream, fd: BorrowedFd<'_>) -> std::io::Result<()> {
    let payload = [HANDOFF_BYTE];
    let mut iov = libc::iovec {
        iov_base: payload.as_ptr().cast_mut().cast(),
        iov_len: 1,
    };
    let mut ctrl = [0u32; CTRL_WORDS];
    // Safety: zeroed msghdr is valid; every pointer below outlives the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl.as_mut_ptr().cast();
    // Safety: CMSG_SPACE/CMSG_LEN are pure size computations; CMSG_FIRSTHDR
    // points into `ctrl`, which is aligned and large enough (checked above).
    unsafe {
        msg.msg_controllen = libc::CMSG_SPACE(FD_LEN);
        let hdr = libc::CMSG_FIRSTHDR(&raw const msg);
        (*hdr).cmsg_level = libc::SOL_SOCKET;
        (*hdr).cmsg_type = libc::SCM_RIGHTS;
        (*hdr).cmsg_len = libc::CMSG_LEN(FD_LEN);
        std::ptr::write_unaligned(libc::CMSG_DATA(hdr).cast::<RawFd>(), fd.as_raw_fd());
    }
    loop {
        // Safety: `msg` and the buffers it references are valid for the call.
        let sent = unsafe { libc::sendmsg(stream.as_raw_fd(), &raw const msg, 0) };
        if sent >= 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn recv_fd(stream: &UnixStream) -> std::io::Result<OwnedFd> {
    let mut payload = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: 1,
    };
    let mut ctrl = [0u32; CTRL_WORDS];
    // Safety: zeroed msghdr is valid; every pointer below outlives the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &raw mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = ctrl.as_mut_ptr().cast();
    // Safety: pure size computation.
    msg.msg_controllen = unsafe { libc::CMSG_SPACE(FD_LEN) };
    let received = loop {
        // Safety: `msg` and the buffers it references are valid for the call.
        let received = unsafe { libc::recvmsg(stream.as_raw_fd(), &raw mut msg, 0) };
        if received >= 0 {
            break received;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != ErrorKind::Interrupted {
            return Err(error);
        }
    };
    if received == 0 {
        return Err(std::io::Error::new(
            ErrorKind::UnexpectedEof,
            "the helper closed the socket without sending a descriptor",
        ));
    }
    if msg.msg_flags & libc::MSG_CTRUNC != 0 {
        return Err(std::io::Error::other(
            "fd control message truncated (unexpected extra control data)",
        ));
    }
    if payload[0] != HANDOFF_BYTE {
        return Err(std::io::Error::other("unexpected handoff payload byte"));
    }
    let mut fd: Option<RawFd> = None;
    // Safety: the CMSG_* walk stays within `ctrl` as populated by recvmsg.
    unsafe {
        let mut hdr = libc::CMSG_FIRSTHDR(&raw const msg);
        while !hdr.is_null() {
            if (*hdr).cmsg_level == libc::SOL_SOCKET && (*hdr).cmsg_type == libc::SCM_RIGHTS {
                fd = Some(std::ptr::read_unaligned(
                    libc::CMSG_DATA(hdr).cast::<RawFd>(),
                ));
                break;
            }
            hdr = libc::CMSG_NXTHDR(&raw const msg, hdr);
        }
    }
    let Some(fd) = fd else {
        return Err(std::io::Error::other(
            "no SCM_RIGHTS control message in the handoff",
        ));
    };
    // Safety: SCM_RIGHTS installed a fresh descriptor this process now owns.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // The kernel does not set CLOEXEC on descriptors delivered via SCM_RIGHTS.
    // Safety: `fd` is owned and valid.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(fd)
}

// The `euid`/`egid` pair is getpeereid's own vocabulary.
#[allow(clippy::similar_names)]
fn peer_euid(stream: &UnixStream) -> std::io::Result<u32> {
    let mut euid: libc::uid_t = 0;
    let mut egid: libc::gid_t = 0;
    // Safety: valid socket fd and out-pointers; getpeereid fills both on success.
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &raw mut euid, &raw mut egid) } == 0 {
        Ok(euid)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;

    use super::{
        DiskKind, DiskList, device_id, disk_kind, mounted_in, peer_euid, recv_fd, send_fd,
        serve_open_block_device,
    };

    #[test]
    fn device_id_accepts_disk_nodes() {
        for (path, id) in [
            ("/dev/disk4", "disk4"),
            ("/dev/disk4s1", "disk4s1"),
            ("/dev/disk4s1s1", "disk4s1s1"),
            ("/dev/rdisk4", "disk4"),
            ("/dev/rdisk10s2", "disk10s2"),
        ] {
            assert_eq!(
                device_id(Path::new(path)).expect("valid disk node"),
                id,
                "{path}"
            );
        }
    }

    #[test]
    fn device_id_rejects_everything_else() {
        for path in [
            "/dev/null",
            "/dev/fd/3",
            "/dev/disk",
            "/dev/rdisk",
            "/dev/disk4s",
            "/dev/disk4x1",
            "/dev/disk4/",
            "/dev/../etc/passwd",
            "/tmp/disk4",
            "disk4",
            "",
        ] {
            assert!(device_id(Path::new(path)).is_err(), "{path:?}");
        }
    }

    /// A trimmed real-machine `diskutil list -plist` (as JSON): a physical
    /// disk0 whose slice disk0s2 backs the synthesized APFS container disk3
    /// (root filesystem), plus an unrelated, unmounted disk6 (a ramdisk).
    fn fixture() -> DiskList {
        serde_json::from_str(
            r#"{"AllDisksAndPartitions":[
                {"DeviceIdentifier":"disk0","Partitions":[
                    {"DeviceIdentifier":"disk0s1"},
                    {"DeviceIdentifier":"disk0s2"}]},
                {"DeviceIdentifier":"disk3",
                 "APFSPhysicalStores":[{"DeviceIdentifier":"disk0s2"}],
                 "APFSVolumes":[
                    {"DeviceIdentifier":"disk3s1",
                     "MountedSnapshots":[{"SnapshotBSD":"disk3s1s1","SnapshotMountPoint":"/"}]},
                    {"DeviceIdentifier":"disk3s1s1","MountPoint":"/"},
                    {"DeviceIdentifier":"disk3s5","MountPoint":"/System/Volumes/Data"}]},
                {"DeviceIdentifier":"disk6","Partitions":[
                    {"DeviceIdentifier":"disk6s1"}]}
            ]}"#,
        )
        .expect("fixture parses")
    }

    #[test]
    fn mounted_sees_apfs_container_through_its_physical_store() {
        let list = fixture();
        // The physical disk (and the exact backing slice) host `/` through the
        // synthesized container even though no mount names disk0 directly.
        for target in ["disk0", "disk0s2"] {
            let mounts = mounted_in(&list, target);
            assert!(
                mounts.iter().any(|m| m.ends_with("at /")),
                "{target}: {mounts:?}"
            );
        }
        // A sibling slice that stores nothing mounted stays attachable.
        assert!(mounted_in(&list, "disk0s1").is_empty());
    }

    #[test]
    fn mounted_reports_direct_and_snapshot_mounts() {
        let list = fixture();
        let mounts = mounted_in(&list, "disk3s1");
        assert!(
            mounts.iter().any(|m| m.contains("(snapshot) at /")),
            "{mounts:?}"
        );
        let mounts = mounted_in(&list, "disk3s5");
        assert_eq!(mounts, ["disk3s5 at /System/Volumes/Data"]);
    }

    #[test]
    fn mounted_ignores_unrelated_disks() {
        let list = fixture();
        assert!(mounted_in(&list, "disk6").is_empty());
        // Prefix must respect slice boundaries: disk0 does not cover disk6.
        assert!(mounted_in(&list, "disk").is_empty());
    }

    #[test]
    fn disk_kind_stats_not_sniffs() {
        let file = tempfile::NamedTempFile::new().expect("temp file");
        assert!(matches!(
            disk_kind(file.path()).expect("regular file"),
            DiskKind::Image
        ));
        // /dev/null is a char device: it classifies as a block-device disk,
        // and then the device-node grammar rejects it loudly.
        assert!(matches!(
            disk_kind(Path::new("/dev/null")).expect("char device"),
            DiskKind::BlockDevice
        ));
        assert!(device_id(Path::new("/dev/null")).is_err());
    }

    #[test]
    fn fd_handoff_roundtrip() {
        let (sender, receiver) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        let mut file = tempfile::tempfile().expect("temp file");
        file.write_all(b"vmkit fd handoff").expect("write marker");
        let sent_meta = file.metadata().expect("metadata");
        send_fd(&sender, std::os::fd::AsFd::as_fd(&file)).expect("send");
        let fd = recv_fd(&receiver).expect("receive");
        let mut roundtripped = std::fs::File::from(fd);
        let got_meta = roundtripped.metadata().expect("metadata");
        // Same open file description, delivered as a new descriptor.
        assert_eq!(got_meta.ino(), sent_meta.ino());
        assert_eq!(got_meta.dev(), sent_meta.dev());
        let mut contents = String::new();
        std::io::Seek::seek(&mut roundtripped, std::io::SeekFrom::Start(0)).expect("seek");
        roundtripped.read_to_string(&mut contents).expect("read");
        assert_eq!(contents, "vmkit fd handoff");
    }

    #[test]
    fn peer_euid_reports_socketpair_peer() {
        let (a, _b) = std::os::unix::net::UnixStream::pair().expect("socketpair");
        // Safety: geteuid cannot fail.
        let me = unsafe { libc::geteuid() };
        assert_eq!(peer_euid(&a).expect("getpeereid"), me);
    }

    #[test]
    fn helper_refuses_to_run_unprivileged() {
        // The test runner is not root (nix sandbox builders never are), so the
        // helper's first gate must trip before it touches anything.
        let error = serve_open_block_device(Path::new("/dev/disk0"), Path::new("/tmp/x.sock"))
            .expect_err("must refuse");
        assert!(error.to_string().contains("must run as root"), "{error}");
    }
}
