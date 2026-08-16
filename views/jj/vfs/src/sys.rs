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

//! The six host operations the writable layer needs that `std` spells
//! differently per platform.
//!
//! Collected here rather than spread through [`crate::overlay`] as `#[cfg]`
//! arms, so that reading the overlay does not mean reading it twice. Each
//! function is one `std` call on each platform; the only ones with logic are
//! the positional read and write, because Windows has no "exact" variant.
//!
//! The Windows arm is compiled by CI and not exercised by it. `jj fs mount` is
//! unix-only, so there is no way to reach a writable layer on Windows and no
//! test can drive one. What that buys is that the crate keeps building there,
//! which is what lets the NFS and snapshot tests keep running on Windows
//! runners; it is not a claim that a Windows overlay works.

use std::fs;
use std::fs::File;
use std::fs::Metadata;
use std::io;
use std::path::Path;
use std::path::PathBuf;

/// Reads exactly `buf.len()` bytes starting at `offset`, without moving the
/// file's own cursor.
#[cfg(unix)]
pub(crate) fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt as _;
    file.read_exact_at(buf, offset)
}

#[cfg(windows)]
pub(crate) fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt as _;
    // `seek_read` is the positional read, but unlike the unix `read_exact_at`
    // it is allowed to return short, so the loop is ours to write.
    let mut rest = buf;
    let mut at = offset;
    while !rest.is_empty() {
        match file.seek_read(rest, at) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "failed to fill whole buffer",
                ));
            }
            Ok(read) => {
                let taken = rest;
                rest = &mut taken[read..];
                at += u64::try_from(read).expect("a partial read fits u64");
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Writes all of `buf` starting at `offset`, without moving the file's own
/// cursor.
#[cfg(unix)]
pub(crate) fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::unix::fs::FileExt as _;
    file.write_all_at(buf, offset)
}

#[cfg(windows)]
pub(crate) fn write_all_at(file: &File, buf: &[u8], offset: u64) -> io::Result<()> {
    use std::os::windows::fs::FileExt as _;
    let mut rest = buf;
    let mut at = offset;
    while !rest.is_empty() {
        match file.seek_write(rest, at) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "failed to write whole buffer",
                ));
            }
            Ok(written) => {
                rest = &rest[written..];
                at += u64::try_from(written).expect("a partial write fits u64");
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

/// Applies POSIX permission bits to an existing path.
#[cfg(unix)]
pub(crate) fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777))
}

#[cfg(windows)]
pub(crate) fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    // The owner write bit is the only part of a POSIX mode NTFS can hold. The
    // read and execute bits have no representation, so they are dropped rather
    // than approximated with an ACL edit that would not round-trip.
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    fs::set_permissions(path, permissions)
}

/// Whether a file in the writable layer should be reported as executable.
#[cfg(unix)]
pub(crate) fn is_executable(metadata: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    metadata.mode() & 0o111 != 0
}

#[cfg(windows)]
pub(crate) fn is_executable(metadata: &Metadata) -> bool {
    // Windows decides executability by extension, not by a bit on the file, so
    // a tree's executable flag has nowhere to live and nothing to be read back
    // from. Every file copied up reports non-executable.
    let _ = metadata;
    false
}

/// Creates a symbolic link at `link` pointing at `target`.
#[cfg(unix)]
pub(crate) fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
pub(crate) fn symlink(target: &Path, link: &Path) -> io::Result<()> {
    // Windows needs to be told at creation time whether the link names a
    // directory, and gets it wrong for the life of the link if it is not.
    // Resolving the target relative to the link's own directory is what the
    // kernel will do when the link is followed.
    let resolved = match link.parent() {
        Some(parent) => parent.join(target),
        None => target.to_path_buf(),
    };
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// A path from the bytes a jj tree stores for a symlink target.
#[cfg(unix)]
pub(crate) fn path_from_bytes(bytes: Vec<u8>) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt as _;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(windows)]
pub(crate) fn path_from_bytes(bytes: Vec<u8>) -> io::Result<PathBuf> {
    // A jj symlink target is arbitrary bytes and a Windows path is UTF-16, so
    // a target that is not UTF-8 has no Windows spelling. Refused rather than
    // lossily transliterated, which would create a link to the wrong place.
    let text =
        String::from_utf8(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    Ok(PathBuf::from(text))
}

/// The bytes a jj tree would store for a symlink target.
///
/// `into_encoded_bytes` is the platform's own encoding: exactly the path bytes
/// on unix, and WTF-8 on Windows, which equals UTF-8 for any path that came
/// back from [`path_from_bytes`].
pub(crate) fn path_into_bytes(path: PathBuf) -> Vec<u8> {
    path.into_os_string().into_encoded_bytes()
}
