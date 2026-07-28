//! Moving the selected files, and removing the ones the destination should no
//! longer have.
//!
//! Two destinations, one file set. A local destination is copied directly; a
//! remote one is streamed as a tar into `ssh <host> tar -x`, so the remote needs
//! nothing installed beyond `tar`, `find` and a shell.
//!
//! Whole files are sent, not deltas. That is the one thing rsync does better,
//! and it is a deliberate v1 trade: the correctness bug this tool replaces was
//! in rsync's *selection*, not its transfer. An unchanged file is skipped by
//! size and mtime, the same quick check rsync uses by default, so a repeat sync
//! moves only what changed even though a changed file moves whole.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, UNIX_EPOCH};

use color_eyre::Result;
use color_eyre::eyre::{WrapErr as _, eyre};

use crate::tree::Entry;

/// What one file at the destination looks like right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stat {
    /// Size in bytes.
    pub size: u64,
    /// Modification time, seconds since the epoch.
    pub mtime: i64,
}

/// The destination's current contents, keyed by path relative to its root.
pub type Manifest = HashMap<PathBuf, Stat>;

/// The outcome of a sync, for the run summary.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Moved {
    /// Files written at the destination.
    pub files: usize,
    /// Bytes those files hold.
    pub bytes: u64,
    /// Files already current, so not sent.
    pub unchanged: usize,
    /// Set when the destination could not report what it already has, so every
    /// file was sent rather than only the changed ones.
    pub manifest_unavailable: bool,
}

/// mtime slack, in seconds, when deciding whether a file is already current.
///
/// A tar extract and a filesystem copy both round mtimes, and some filesystems
/// only carry one second of resolution, so an exact compare re-sends files that
/// are identical.
const MTIME_SLACK: i64 = 1;

/// Whether `entry` has to be sent, given what the destination already has.
///
/// Symlinks always go: they are a few bytes and their target is not in the
/// manifest, so there is nothing to compare.
#[must_use]
pub fn needs_send(entry: &Entry, existing: Option<Stat>) -> bool {
    if entry.symlink {
        return true;
    }
    existing.is_none_or(|stat| {
        stat.size != entry.size || (stat.mtime - entry.mtime).abs() > MTIME_SLACK
    })
}

/// Reject any destination-relative path that could act outside the destination
/// root.
///
/// Every path acted on comes from the destination's own listing, so this guards
/// against a hostile or corrupt listing rather than against the caller: an
/// absolute path, or one climbing through `..`, would let a `--delete` reach the
/// wider filesystem.
///
/// # Errors
/// Returns an error if the path is empty, absolute, or contains anything other
/// than plain names.
pub fn confine(relative: &Path) -> Result<()> {
    if relative.as_os_str().is_empty() {
        return Err(eyre!("refusing to act on an empty destination path"));
    }
    for component in relative.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(eyre!(
                "refusing to act on {}: a destination path must be relative \
                 and must not climb out through ..",
                relative.display()
            ));
        }
    }
    Ok(())
}

/// The paths present at the destination that the source no longer has.
///
/// # Errors
/// Returns an error if the destination reported a path that escapes its own
/// root.
pub fn plan_deletions<S: std::hash::BuildHasher>(
    present: &Manifest,
    keep: &HashSet<PathBuf, S>,
) -> Result<Vec<PathBuf>> {
    let mut doomed: Vec<PathBuf> = Vec::new();
    for path in present.keys() {
        if keep.contains(path) {
            continue;
        }
        confine(path)?;
        doomed.push(path.clone());
    }
    doomed.sort();
    Ok(doomed)
}

/// List a local destination directory.
///
/// # Errors
/// Returns an error if a directory under `dest` cannot be read.
pub fn local_manifest(dest: &Path) -> Result<Manifest> {
    let mut manifest = Manifest::new();
    if !dest.is_dir() {
        return Ok(manifest);
    }
    collect_local(dest, Path::new(""), &mut manifest)?;
    Ok(manifest)
}

fn collect_local(dir: &Path, prefix: &Path, manifest: &mut Manifest) -> Result<()> {
    let listing =
        std::fs::read_dir(dir).wrap_err_with(|| format!("could not read {}", dir.display()))?;
    for item in listing {
        let item = item.wrap_err_with(|| format!("could not read an entry in {}", dir.display()))?;
        let metadata = item
            .path()
            .symlink_metadata()
            .wrap_err_with(|| format!("could not stat {}", item.path().display()))?;
        let relative = prefix.join(item.file_name());
        // Directory symlinks are not followed: descending through one would let
        // a listing, and therefore a --delete, leave the destination root.
        if metadata.is_dir() {
            collect_local(&item.path(), &relative, manifest)?;
            continue;
        }
        manifest.insert(
            relative,
            Stat {
                size: if metadata.is_symlink() {
                    0
                } else {
                    metadata.len()
                },
                mtime: mtime_seconds(&metadata),
            },
        );
    }
    Ok(())
}

fn mtime_seconds(metadata: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt as _;
    metadata.mtime()
}

/// An mtime as seconds since the epoch, refusing rather than defaulting on a
/// pre-epoch stamp: silently rewriting it to 0 would make the file look
/// permanently out of date on every later sync.
fn epoch_seconds(mtime: i64, source: &Path) -> Result<u64> {
    u64::try_from(mtime)
        .wrap_err_with(|| format!("{} is stamped before the epoch", source.display()))
}

/// Copy the entries that are not already current into a local destination.
///
/// # Errors
/// Returns an error if the destination cannot be created or written.
pub fn push_local(
    root: &Path,
    entries: &[Entry],
    dest: &Path,
    force: bool,
    dry_run: bool,
) -> Result<Moved> {
    let manifest = local_manifest(dest)?;
    let mut moved = Moved::default();

    for entry in entries {
        if !force && !needs_send(entry, manifest.get(&entry.relative).copied()) {
            moved.unchanged += 1;
            continue;
        }
        moved.files += 1;
        moved.bytes += entry.size;
        if dry_run {
            continue;
        }
        write_local(root, entry, dest)?;
    }

    Ok(moved)
}

fn write_local(root: &Path, entry: &Entry, dest: &Path) -> Result<()> {
    let source = root.join(&entry.relative);
    let destination = dest.join(&entry.relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("could not create {}", parent.display()))?;
    }

    if entry.symlink {
        let link = std::fs::read_link(&source)
            .wrap_err_with(|| format!("could not read the symlink {}", source.display()))?;
        // A symlink cannot be overwritten in place.
        if destination.symlink_metadata().is_ok() {
            std::fs::remove_file(&destination)
                .wrap_err_with(|| format!("could not replace {}", destination.display()))?;
        }
        std::os::unix::fs::symlink(&link, &destination)
            .wrap_err_with(|| format!("could not link {}", destination.display()))?;
        return Ok(());
    }

    std::fs::copy(&source, &destination).wrap_err_with(|| {
        format!(
            "could not copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;

    // Carry the mtime across, or every later sync re-sends every file.
    let modified = UNIX_EPOCH
        .checked_add(Duration::from_secs(epoch_seconds(entry.mtime, &source)?))
        .ok_or_else(|| eyre!("{} has an unrepresentable mtime", source.display()))?;
    let handle = std::fs::File::options()
        .write(true)
        .open(&destination)
        .wrap_err_with(|| format!("could not reopen {}", destination.display()))?;
    handle
        .set_modified(modified)
        .wrap_err_with(|| format!("could not set the mtime on {}", destination.display()))?;

    Ok(())
}

/// Remove local destination paths the source no longer has.
///
/// # Errors
/// Returns an error if the destination reported an escaping path, or a file
/// cannot be removed.
pub fn delete_local(dest: &Path, doomed: &[PathBuf], dry_run: bool) -> Result<()> {
    for relative in doomed {
        confine(relative)?;
        if dry_run {
            continue;
        }
        let path = dest.join(relative);
        std::fs::remove_file(&path)
            .wrap_err_with(|| format!("could not remove {}", path.display()))?;
    }
    Ok(())
}

/// How to reach a remote destination.
#[derive(Debug, Clone)]
pub struct Remote {
    /// The program that carries a command to the far end. `ssh` unless
    /// `TREE_SYNC_SSH` names something else, which is how an ssh wrapper, or a
    /// test double, stands in without the code caring.
    pub program: String,
    /// Anything ssh accepts as a destination: `host`, `user@host`, or a
    /// `~/.ssh/config` alias.
    pub host: String,
    /// Extra `-o` options.
    pub options: Vec<String>,
}

impl Remote {
    /// A remote reached with the configured ssh program.
    #[must_use]
    pub fn new(host: impl Into<String>, options: Vec<String>) -> Self {
        Self {
            program: std::env::var("TREE_SYNC_SSH").unwrap_or_else(|_| "ssh".to_owned()),
            host: host.into(),
            options,
        }
    }

    /// An invocation that will run one shell command on the far end.
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        // Whole files go over the wire, so let ssh compress them.
        command.arg("-C");
        for option in &self.options {
            command.arg("-o").arg(option);
        }
        command.arg(&self.host);
        command
    }
}

/// Quote a path for a remote `sh -c` body.
fn quote(path: &Path) -> Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| eyre!("{} is not valid UTF-8, which ssh needs", path.display()))?;
    Ok(shlex::try_quote(text)
        .wrap_err_with(|| format!("{text} cannot be passed through a shell"))?
        .into_owned())
}

/// Exit code the remote manifest script uses for "the destination is not there
/// yet", which is a normal first sync rather than a failure.
const REMOTE_DEST_MISSING: i32 = 3;

impl Remote {
    /// Ask the remote what it already has.
    ///
    /// Returns `None` when the destination cannot report itself, which happens
    /// on a `find` without GNU's `-printf`. The caller says so and sends
    /// everything, rather than quietly skipping files it could not compare.
    ///
    /// # Errors
    /// Returns an error if ssh itself cannot be run.
    pub fn manifest(&self, dest: &Path) -> Result<Option<Manifest>> {
        let quoted = quote(dest)?;
        let script = format!(
            "if [ ! -d {quoted} ]; then exit {REMOTE_DEST_MISSING}; fi; \
             cd {quoted} && find . \\( -type f -o -type l \\) -printf '%s\\t%T@\\t%p\\n'"
        );
        let output = self
            .command()
            .arg(script)
            .output()
            .wrap_err_with(|| format!("could not run {}", self.program))?;

        if output.status.code() == Some(REMOTE_DEST_MISSING) {
            return Ok(Some(Manifest::new()));
        }
        if !output.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Ok(Some(parse_find_manifest(&text)))
    }
}

/// Parse `find . -printf '%s\t%T@\t%p\n'` output.
#[must_use]
pub fn parse_find_manifest(text: &str) -> Manifest {
    let mut manifest = Manifest::new();
    for line in text.lines() {
        let mut fields = line.splitn(3, '\t');
        let (Some(size), Some(mtime), Some(path)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let Ok(size) = size.parse::<u64>() else {
            continue;
        };
        // `%T@` is `<seconds>.<fraction>`; the seconds are all this compares.
        let Ok(mtime) = mtime.split('.').next().unwrap_or(mtime).parse::<i64>() else {
            continue;
        };
        let relative = path.strip_prefix("./").unwrap_or(path);
        if relative.is_empty() {
            continue;
        }
        manifest.insert(PathBuf::from(relative), Stat { size, mtime });
    }
    manifest
}

impl Remote {
    /// Stream the entries that are not already current to a remote destination.
    ///
    /// # Errors
    /// Returns an error if ssh or the remote `tar` fails.
    pub fn push(
        &self,
        root: &Path,
        entries: &[Entry],
        dest: &Path,
        force: bool,
        dry_run: bool,
    ) -> Result<Moved> {
        let manifest = self.manifest(dest)?;
        let mut moved = Moved {
            manifest_unavailable: manifest.is_none(),
            ..Moved::default()
        };

        let sending: Vec<&Entry> = entries
            .iter()
            .filter(|entry| {
                let existing = manifest
                    .as_ref()
                    .and_then(|known| known.get(&entry.relative).copied());
                let send = force || manifest.is_none() || needs_send(entry, existing);
                if !send {
                    moved.unchanged += 1;
                }
                send
            })
            .collect();

        moved.files = sending.len();
        moved.bytes = sending.iter().map(|entry| entry.size).sum();

        if dry_run || sending.is_empty() {
            return Ok(moved);
        }

        let quoted = quote(dest)?;
        let script = format!("mkdir -p {quoted} && tar -x -p -f - -C {quoted}");
        let mut child = self
            .command()
            .arg(script)
            .stdin(Stdio::piped())
            .spawn()
            .wrap_err_with(|| format!("could not run {}", self.program))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| eyre!("{} gave us no stdin for the archive", self.program))?;

        let archive = write_archive(root, &sending, stdin);
        let status = child
            .wait()
            .wrap_err_with(|| format!("could not wait for {}", self.program))?;
        archive?;
        if !status.success() {
            return Err(eyre!(
                "{} {} exited with {status} while extracting the archive",
                self.program,
                self.host
            ));
        }

        Ok(moved)
    }
}

fn write_archive(root: &Path, entries: &[&Entry], sink: impl std::io::Write) -> Result<()> {
    let mut builder = tar::Builder::new(sink);
    // Symlinks are written as links below; following them here would copy the
    // target's bytes to a path that should hold a pointer.
    builder.follow_symlinks(false);

    for entry in entries {
        let source = root.join(&entry.relative);
        if entry.symlink {
            let link = std::fs::read_link(&source)
                .wrap_err_with(|| format!("could not read the symlink {}", source.display()))?;
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_mode(entry.mode);
            header.set_mtime(epoch_seconds(entry.mtime, &source)?);
            builder
                .append_link(&mut header, &entry.relative, &link)
                .wrap_err_with(|| format!("could not archive the symlink {}", source.display()))?;
            continue;
        }
        builder
            .append_path_with_name(&source, &entry.relative)
            .wrap_err_with(|| format!("could not archive {}", source.display()))?;
    }

    builder.finish().wrap_err("could not finish the archive")?;
    Ok(())
}

impl Remote {
    /// Remove remote destination paths the source no longer has.
    ///
    /// The remote command changes into the destination first and is handed only
    /// relative paths, every one of which passed [`confine`], so nothing
    /// outside the destination root is reachable even if the listing had been
    /// tampered with.
    ///
    /// # Errors
    /// Returns an error if a path escapes the destination, or ssh fails.
    pub fn delete(&self, dest: &Path, doomed: &[PathBuf], dry_run: bool) -> Result<()> {
        for relative in doomed {
            confine(relative)?;
        }
        if dry_run || doomed.is_empty() {
            return Ok(());
        }

        let quoted = quote(dest)?;
        let script = format!("cd {quoted} && xargs -0 rm -f --");
        let mut child = self
            .command()
            .arg(script)
            .stdin(Stdio::piped())
            .spawn()
            .wrap_err_with(|| format!("could not run {}", self.program))?;
        {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| eyre!("{} gave us no stdin for the delete list", self.program))?;
            for relative in doomed {
                let text = relative
                    .to_str()
                    .ok_or_else(|| eyre!("{} is not valid UTF-8", relative.display()))?;
                stdin
                    .write_all(text.as_bytes())
                    .and_then(|()| stdin.write_all(&[0]))
                    .wrap_err("could not send the delete list")?;
            }
        }
        let status = child
            .wait()
            .wrap_err_with(|| format!("could not wait for {}", self.program))?;
        if !status.success() {
            return Err(eyre!(
                "{} {} exited with {status} while deleting",
                self.program,
                self.host
            ));
        }
        Ok(())
    }
}

/// The destination's current contents, however it has to be asked.
///
/// # Errors
/// Returns an error if the destination cannot be listed.
pub fn manifest_for(remote: Option<&Remote>, dest: &Path) -> Result<Option<Manifest>> {
    remote.map_or_else(
        || local_manifest(dest).map(Some),
        |remote| remote.manifest(dest),
    )
}

#[cfg(test)]
mod tests {
    use super::{Manifest, Stat, confine, needs_send, parse_find_manifest, plan_deletions};
    use crate::tree::Entry;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    fn entry(path: &str, size: u64, mtime: i64) -> Entry {
        Entry {
            relative: PathBuf::from(path),
            size,
            symlink: false,
            mode: 0o644,
            mtime,
        }
    }

    #[test]
    fn an_unchanged_file_is_not_resent() {
        let file = entry("src/main.rs", 120, 1_700_000_000);
        assert!(!needs_send(
            &file,
            Some(Stat {
                size: 120,
                mtime: 1_700_000_000,
            })
        ));
        assert!(needs_send(
            &file,
            Some(Stat {
                size: 121,
                mtime: 1_700_000_000,
            })
        ));
        assert!(needs_send(
            &file,
            Some(Stat {
                size: 120,
                mtime: 1_700_000_600,
            })
        ));
        assert!(needs_send(&file, None));
    }

    #[test]
    fn a_destination_path_may_not_escape_its_root() {
        assert!(confine(Path::new("src/main.rs")).is_ok());
        assert!(confine(Path::new("")).is_err());
        assert!(confine(Path::new("/etc/passwd")).is_err());
        assert!(confine(Path::new("../outside")).is_err());
        assert!(confine(Path::new("a/../../outside")).is_err());
    }

    #[test]
    fn deletions_are_the_destination_minus_the_source() {
        let mut present = Manifest::new();
        for path in ["keep.rs", "stale.rs", "sub/gone.rs"] {
            present.insert(
                PathBuf::from(path),
                Stat {
                    size: 1,
                    mtime: 0,
                },
            );
        }
        let keep: HashSet<PathBuf> = std::iter::once(PathBuf::from("keep.rs")).collect();

        let doomed = plan_deletions(&present, &keep).expect("plans");
        assert_eq!(
            doomed,
            vec![PathBuf::from("stale.rs"), PathBuf::from("sub/gone.rs")]
        );
    }

    #[test]
    fn an_escaping_destination_listing_is_refused_rather_than_deleted() {
        let mut present = Manifest::new();
        present.insert(
            PathBuf::from("../../etc/passwd"),
            Stat { size: 1, mtime: 0 },
        );
        let error = plan_deletions(&present, &HashSet::new()).expect_err("refuses");
        assert!(
            error.to_string().contains("must not climb out"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn find_output_parses_into_a_manifest() {
        let manifest = parse_find_manifest(
            "120\t1700000000.1234567890\t./src/main.rs\n\
             7\t1700000001.0000000000\t./link\n\
             garbage line\n",
        );
        assert_eq!(
            manifest.get(Path::new("src/main.rs")).copied(),
            Some(Stat {
                size: 120,
                mtime: 1_700_000_000,
            })
        );
        assert_eq!(
            manifest.get(Path::new("link")).copied(),
            Some(Stat {
                size: 7,
                mtime: 1_700_000_001,
            })
        );
        assert_eq!(manifest.len(), 2);
    }
}
