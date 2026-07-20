//! Resolve the pts device of the current ix-term session.
//!
//! Order: an explicit `IX_TERM_SESSION_ID` names the session and maps to a
//! pts through the server-maintained `<sessions root>/<id>/pts` file; without
//! it, walk `/proc` ancestry from the parent upward until an ancestor has a
//! controlling pts. The two are alternatives, not fallbacks: a session id
//! that does not resolve is an error, never a reason to try ancestry.

// Only the linux build reaches this from `main`; unit tests exercise it on
// every platform (the walk is plain file reading over an injectable root).
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Unix98 pty slave majors, `include/uapi/linux/major.h`:
/// `UNIX98_PTY_SLAVE_MAJOR` (136) spanning `UNIX98_PTY_MAJOR_COUNT` (8).
const PTY_SLAVE_MAJOR_FIRST: u32 = 136;
const PTY_SLAVE_MAJOR_COUNT: u32 = 8;

/// Upper bound on ancestry hops: a deeper chain means a corrupt or cyclic
/// ppid graph in the fixture or a pathological process tree, not a session.
const MAX_ANCESTRY_HOPS: u32 = 128;

pub struct Resolution {
    pub session_id: Option<OsString>,
    pub sessions_root: PathBuf,
    pub proc_root: PathBuf,
    pub start_pid: u32,
}

pub fn resolve(resolution: &Resolution) -> Result<PathBuf> {
    resolution.session_id.as_ref().map_or_else(
        || ancestry_pts(&resolution.proc_root, resolution.start_pid),
        |id| session_pts(&resolution.sessions_root, id),
    )
}

fn session_pts(sessions_root: &Path, id: &OsStr) -> Result<PathBuf> {
    let id = id.to_str().context("IX_TERM_SESSION_ID is not UTF-8")?;
    if id.is_empty() || id == "." || id == ".." || id.contains(['/', '\\']) {
        bail!("invalid IX_TERM_SESSION_ID {id:?}");
    }
    if !sessions_root.is_dir() {
        bail!(
            "IX_TERM_SESSION_ID is set but {} does not exist; \
             is the ix-term server running on this host?",
            sessions_root.display(),
        );
    }

    let pts_file = sessions_root.join(id).join("pts");
    let contents = std::fs::read_to_string(&pts_file).with_context(|| {
        format!("session {id:?} has no pts mapping at {}", pts_file.display())
    })?;
    let pts = contents.trim();
    if !pts.starts_with('/') {
        bail!(
            "{} does not contain an absolute pts path (got {pts:?})",
            pts_file.display(),
        );
    }
    Ok(PathBuf::from(pts))
}

fn ancestry_pts(proc_root: &Path, start_pid: u32) -> Result<PathBuf> {
    let mut pid = start_pid;
    let mut walked = Vec::new();
    for _ in 0..MAX_ANCESTRY_HOPS {
        let stat_path = proc_root.join(pid.to_string()).join("stat");
        let stat = std::fs::read_to_string(&stat_path)
            .with_context(|| format!("cannot read {}", stat_path.display()))?;
        let fields = parse_stat(&stat)
            .with_context(|| format!("cannot parse {}", stat_path.display()))?;

        if let Some(index) = pts_index(fields.tty_nr) {
            return Ok(PathBuf::from(format!("/dev/pts/{index}")));
        }

        walked.push(pid);
        if pid == 1 || fields.ppid == 0 {
            break;
        }
        pid = fields.ppid;
    }
    bail!(
        "no ancestor has a controlling pts (walked pids {walked:?}); \
         run inside an ix-term session or set IX_TERM_SESSION_ID",
    )
}

struct StatFields {
    ppid: u32,
    tty_nr: i32,
}

/// Parse the `/proc/<pid>/stat` fields the walk needs. `comm` (field 2) is an
/// unescaped process name that may itself contain spaces and `)`, so split at
/// the last `)` per proc(5).
fn parse_stat(stat: &str) -> Result<StatFields> {
    let (_, after_comm) = stat.rsplit_once(')').context("no `)` after comm")?;
    // Fields after comm: state(3) ppid(4) pgrp(5) session(6) tty_nr(7).
    let mut fields = after_comm.split_whitespace();
    let ppid = fields.nth(1).context("missing ppid field")?;
    let tty_nr = fields.nth(2).context("missing tty_nr field")?;
    Ok(StatFields {
        ppid: ppid.parse().with_context(|| format!("ppid {ppid:?} is not a pid"))?,
        tty_nr: tty_nr
            .parse()
            .with_context(|| format!("tty_nr {tty_nr:?} is not an integer"))?,
    })
}

/// Decode `tty_nr` — a `new_encode_dev` packed `dev_t`: minor low byte in bits
/// 0-7, major in bits 8-19, minor high bits in 20-31 — and return the pts
/// index when the device is a Unix98 pty slave.
fn pts_index(tty_nr: i32) -> Option<u32> {
    // The kernel prints the packed dev_t as a signed int; undo that, not the
    // packing.
    let dev = tty_nr.cast_unsigned();
    let major = (dev >> 8) & 0xfff;
    let minor = (dev & 0xff) | ((dev >> 12) & 0xff_f00);

    let majors = PTY_SLAVE_MAJOR_FIRST..PTY_SLAVE_MAJOR_FIRST + PTY_SLAVE_MAJOR_COUNT;
    if !majors.contains(&major) {
        return None;
    }
    // Modern devpts keeps every index on major 136 as the minor; majors
    // 137-143 are the legacy 256-per-major layout, which never sees minors
    // >= 256, so one formula covers both.
    Some((major - PTY_SLAVE_MAJOR_FIRST) * 256 + minor)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::{Resolution, ancestry_pts, parse_stat, pts_index, resolve};

    /// Linux `new_encode_dev`, transcribed from `include/linux/kdev_t.h` so
    /// decoding is checked against the kernel's packing, not our own inverse.
    fn new_encode_dev(major: u32, minor: u32) -> i32 {
        ((minor & 0xff) | (major << 8) | ((minor & !0xff) << 12)).cast_signed()
    }

    #[expect(clippy::similar_names, reason = "pid/ppid are the proc(5) field names")]
    fn write_stat(proc_root: &Path, pid: u32, comm: &str, ppid: u32, tty_nr: i32) {
        let dir = proc_root.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("stat"),
            format!("{pid} ({comm}) S {ppid} {pid} {pid} {tty_nr} 0 4194304"),
        )
        .unwrap();
    }

    #[test]
    fn pts_index_decodes_kernel_dev_packing() {
        assert_eq!(pts_index(new_encode_dev(136, 0)), Some(0));
        assert_eq!(pts_index(new_encode_dev(136, 3)), Some(3));
        // 20-bit minors: modern devpts keeps large indexes on major 136.
        assert_eq!(pts_index(new_encode_dev(136, 4096)), Some(4096));
        // Legacy layout spread indexes over majors 137-143, 256 per major.
        assert_eq!(pts_index(new_encode_dev(137, 5)), Some(261));
        // No controlling terminal, a virtual console, a serial adapter.
        assert_eq!(pts_index(0), None);
        assert_eq!(pts_index(new_encode_dev(4, 1)), None);
        assert_eq!(pts_index(new_encode_dev(188, 0)), None);
    }

    #[test]
    fn stat_parsing_survives_hostile_comm() {
        // comm is unescaped: spaces and `)` inside are legal.
        let fields = parse_stat("123 (a ) evil) comm) R 45 100 100 34819 0").unwrap();
        assert_eq!(fields.ppid, 45);
        assert_eq!(fields.tty_nr, 34819);
    }

    #[test]
    fn ancestry_walk_stops_at_first_pts_ancestor() {
        let proc_root = tempfile::tempdir().unwrap();
        write_stat(proc_root.path(), 100, "child proc", 50, 0);
        write_stat(proc_root.path(), 50, "term (v2)", 1, new_encode_dev(136, 6));

        let pts = ancestry_pts(proc_root.path(), 100).unwrap();
        assert_eq!(pts, Path::new("/dev/pts/6"));
    }

    #[test]
    fn ancestry_walk_without_pts_fails() {
        let proc_root = tempfile::tempdir().unwrap();
        write_stat(proc_root.path(), 100, "child", 1, 0);
        write_stat(proc_root.path(), 1, "init", 0, 0);

        let err = ancestry_pts(proc_root.path(), 100).unwrap_err();
        assert!(err.to_string().contains("IX_TERM_SESSION_ID"), "{err}");
    }

    #[test]
    fn session_id_wins_and_never_falls_back_to_ancestry() {
        let sessions_root = tempfile::tempdir().unwrap();
        let session = sessions_root.path().join("abc");
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("pts"), "/dev/pts/9\n").unwrap();

        // A proc root that does not exist proves ancestry is never consulted
        // when the session id resolves.
        let resolved = resolve(&Resolution {
            session_id: Some("abc".into()),
            sessions_root: sessions_root.path().to_path_buf(),
            proc_root: "/nonexistent/proc".into(),
            start_pid: 100,
        })
        .unwrap();
        assert_eq!(resolved, Path::new("/dev/pts/9"));
    }

    #[test]
    fn set_session_id_that_cannot_resolve_is_an_error_not_a_fallback() {
        // A valid pts ancestry exists, but the session id is set and its
        // sessions root is absent: that must fail loudly.
        let proc_root = tempfile::tempdir().unwrap();
        write_stat(proc_root.path(), 100, "child", 1, new_encode_dev(136, 2));

        let err = resolve(&Resolution {
            session_id: Some("abc".into()),
            sessions_root: "/nonexistent/ix-term/sessions".into(),
            proc_root: proc_root.path().to_path_buf(),
            start_pid: 100,
        })
        .unwrap_err();
        assert!(err.to_string().contains("ix-term server"), "{err}");
    }

    #[test]
    fn relative_pts_mapping_is_rejected() {
        let sessions_root = tempfile::tempdir().unwrap();
        let session = sessions_root.path().join("abc");
        fs::create_dir_all(&session).unwrap();
        fs::write(session.join("pts"), "pts/9").unwrap();

        let err = resolve(&Resolution {
            session_id: Some("abc".into()),
            sessions_root: sessions_root.path().to_path_buf(),
            proc_root: "/nonexistent/proc".into(),
            start_pid: 100,
        })
        .unwrap_err();
        assert!(err.to_string().contains("absolute"), "{err}");
    }
}
