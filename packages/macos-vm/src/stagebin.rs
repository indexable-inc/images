//! Stage a nix-built macOS binary so it runs on a vanilla guest.
//!
//! A binary built under Nix links its dynamic libraries by absolute
//! `/nix/store/...` path. Those paths do not exist on a freshly installed macOS
//! guest, so the dynamic linker refuses to start the process ("Library not
//! loaded"). This module copies the binary and rewrites every `/nix/store`
//! dylib reference so the copy depends only on libraries the guest already has,
//! then ad-hoc re-signs it (a Mach-O whose load commands changed must be
//! re-signed or the kernel kills it for an invalid signature).
//!
//! Two rewrite strategies per dependency:
//!
//! - **Repoint to a system library.** macOS ships the common C/C++ runtime
//!   libraries under `/usr/lib` (libiconv, libc++, libobjc, libresolv, libz, …).
//!   When the dependency's basename matches one that exists at the canonical
//!   `/usr/lib/<name>` on this host, rewrite the reference to that path. The
//!   guest has the same system libraries, so the reference resolves there.
//! - **Bundle it.** A dependency with no system equivalent (a third-party dylib
//!   the app itself needs) is copied next to the output and the reference is
//!   rewritten to `@loader_path/<name>`, which the linker resolves relative to
//!   the binary's own directory. The bundled copy is itself re-signed, since it
//!   too is a Mach-O placed at a new path.
//!
//! After rewriting, `otool -L` on the output must show zero `/nix/store` paths;
//! any remaining one is a typed error (no silent partial result).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use snafu::{ResultExt, Snafu};

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("input binary {path:?} does not exist"))]
    MissingInput { path: PathBuf },
    #[snafu(display("output path {path:?} has no parent directory"))]
    NoOutputParent { path: PathBuf },
    #[snafu(display("could not create output directory {path:?}: {source}"))]
    CreateOutputDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("could not copy {from:?} to {to:?}: {source}"))]
    Copy {
        from: PathBuf,
        to: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("could not make {path:?} writable: {source}"))]
    MakeWritable {
        path: PathBuf,
        source: std::io::Error,
    },
    #[snafu(display("{tool} failed to run: {source}"))]
    Spawn {
        tool: &'static str,
        source: std::io::Error,
    },
    #[snafu(display(
        "{tool} exited with status {status}: {stderr}"
    ))]
    Tool {
        tool: &'static str,
        status: String,
        stderr: String,
    },
    #[snafu(display("otool -L output for {path:?} was not valid UTF-8"))]
    OtoolEncoding { path: PathBuf },
    #[snafu(display(
        "a /nix/store dependency {dep:?} has no basename, so it cannot be bundled"
    ))]
    DepNoBasename { dep: String },
    #[snafu(display(
        "staged binary {path:?} still references /nix/store after rewriting:\n{remaining}"
    ))]
    StorePathsRemain { path: PathBuf, remaining: String },
}

/// Stage `input` into `output`: copy it, repoint every `/nix/store` dylib to a
/// system path or a bundled copy, ad-hoc re-sign, and verify no `/nix/store`
/// reference survives. Bundled third-party dylibs are processed transitively
/// (their own `/nix/store` deps are repointed/bundled too) and each is staged in
/// place, so the whole dependency closure is guest-portable. Returns the staged
/// path (`output`).
pub fn stage_binary(input: &Path, output: &Path) -> Result<PathBuf, Error> {
    if !input.exists() {
        return Err(Error::MissingInput { path: input.to_path_buf() });
    }
    let out_dir = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| Error::NoOutputParent { path: output.to_path_buf() })?;
    std::fs::create_dir_all(out_dir).context(CreateOutputDirSnafu { path: out_dir.to_path_buf() })?;

    copy_writable(input, output)?;

    // Work a queue of artifacts. The output is first; bundling a third-party
    // dylib enqueues that bundled copy so its own `/nix/store` deps are processed
    // the same way (transitive closure). `bundled` dedupes by basename so each
    // distinct dylib is copied, processed, and re-signed exactly once.
    let mut bundled: BTreeSet<String> = BTreeSet::new();
    let mut queue: Vec<PathBuf> = vec![output.to_path_buf()];
    while let Some(artifact) = queue.pop() {
        stage_artifact(&artifact, out_dir, &mut bundled, &mut queue)?;
    }
    Ok(output.to_path_buf())
}

/// Make one Mach-O artifact (the output binary or a bundled dylib) guest-portable
/// in place: rewrite its own `/nix/store` install id, repoint or bundle each
/// `/nix/store` load dependency (enqueuing newly bundled dylibs for the same
/// treatment), re-sign, and assert no `/nix/store` reference remains.
fn stage_artifact(
    artifact: &Path,
    out_dir: &Path,
    bundled: &mut BTreeSet<String>,
    queue: &mut Vec<PathBuf>,
) -> Result<(), Error> {
    // A dylib carries its own install id (LC_ID_DYLIB), which `otool -L` lists as
    // the first indented line. For a nix-built dylib that id is a `/nix/store`
    // path, and it must be rewritten with `-id` (not `-change`, which only
    // touches load commands). `otool -D` prints just the id (empty for an
    // executable), so it tells the id apart from the load deps.
    if let Some(id) = install_id(artifact)?
        && id.starts_with("/nix/store/")
    {
        let name = basename(&id).ok_or_else(|| Error::DepNoBasename { dep: id.clone() })?;
        change_id(artifact, &format!("@loader_path/{name}"))?;
    }

    // Repoint or bundle each `/nix/store` load dependency. The id was already
    // rewritten above, so it is no longer a `/nix/store` path here and does not
    // reappear in this list.
    let deps = nix_store_deps(artifact)?;
    for dep in &deps {
        if let Some(system) = system_equivalent(dep) {
            change_dep(artifact, dep, &system)?;
        } else {
            let name = basename(dep).ok_or_else(|| Error::DepNoBasename { dep: dep.clone() })?;
            // Copy the dylib next to the output (once per distinct basename),
            // then enqueue it so its own `/nix/store` deps are staged in turn.
            if bundled.insert(name.clone()) {
                let bundled_path = out_dir.join(&name);
                copy_writable(Path::new(dep), &bundled_path)?;
                queue.push(bundled_path);
            }
            change_dep(artifact, dep, &format!("@loader_path/{name}"))?;
        }
    }

    // The load commands changed, so the prior signature is invalid; re-sign.
    codesign_adhoc(artifact)?;

    // No silent fallback: a surviving `/nix/store` reference means this artifact
    // would not load on a guest, so fail loudly with the offenders. Applied to
    // every artifact, the output and each bundled dylib.
    let remaining = nix_store_deps(artifact)?;
    if !remaining.is_empty() {
        return Err(Error::StorePathsRemain {
            path: artifact.to_path_buf(),
            remaining: remaining.join("\n"),
        });
    }
    Ok(())
}

/// Copy `from` to `to` and make `to` writable (a Nix store source is read-only,
/// and `install_name_tool`/`codesign` must rewrite the copy in place).
fn copy_writable(from: &Path, to: &Path) -> Result<(), Error> {
    std::fs::copy(from, to).context(CopySnafu {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    })?;
    let mut perms = std::fs::metadata(to)
        .context(MakeWritableSnafu { path: to.to_path_buf() })?
        .permissions();
    if perms.readonly() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Preserve the executable bits; just add owner-write.
            let mode = perms.mode() | 0o200;
            perms.set_mode(mode);
        }
        #[cfg(not(unix))]
        {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
        }
        std::fs::set_permissions(to, perms)
            .context(MakeWritableSnafu { path: to.to_path_buf() })?;
    }
    Ok(())
}

/// Run `otool -L <path>` and return the list of `/nix/store/...` dependency
/// paths it reports (the load-command target paths, not the install name line).
fn nix_store_deps(path: &Path) -> Result<Vec<String>, Error> {
    let output = Command::new("/usr/bin/otool")
        .arg("-L")
        .arg(path)
        .output()
        .context(SpawnSnafu { tool: "otool" })?;
    if !output.status.success() {
        return Err(Error::Tool {
            tool: "otool",
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| Error::OtoolEncoding { path: path.to_path_buf() })?;
    // `otool -L` prints the file path, then one indented line per dependency:
    //   /path/to/bin:
    //   \t/nix/store/.../libfoo.dylib (compatibility version ...)
    // Take the first whitespace-delimited token of each indented line.
    Ok(text
        .lines()
        .filter(|line| line.starts_with('\t') || line.starts_with("    "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|tok| tok.starts_with("/nix/store/"))
        .map(str::to_owned)
        .collect())
}

/// The Mach-O install id (`LC_ID_DYLIB`) of `path`, or `None` if it has none (an
/// executable). `otool -D` prints the path then, on the next line, the id; an
/// executable prints only the path line, so there is no second line.
fn install_id(path: &Path) -> Result<Option<String>, Error> {
    let output = Command::new("/usr/bin/otool")
        .arg("-D")
        .arg(path)
        .output()
        .context(SpawnSnafu { tool: "otool" })?;
    if !output.status.success() {
        return Err(Error::Tool {
            tool: "otool",
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|_| Error::OtoolEncoding { path: path.to_path_buf() })?;
    // First line is the file path (with a trailing `:`); the id, if any, is the
    // next non-empty line.
    Ok(text
        .lines()
        .skip(1)
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned))
}

/// Rewrite a dylib's own install id (`LC_ID_DYLIB`) via `install_name_tool -id`.
fn change_id(dylib: &Path, new: &str) -> Result<(), Error> {
    run_checked(
        "install_name_tool",
        Command::new("/usr/bin/install_name_tool")
            .arg("-id")
            .arg(new)
            .arg(dylib),
    )
}

/// Runtime libraries macOS ships under `/usr/lib` and a fresh guest also has.
/// These are the C/C++/system runtimes a nix-built binary commonly links; the
/// guest resolves `/usr/lib/<name>` for each, so a `/nix/store` copy is repointed
/// rather than bundled.
///
/// macOS 11+ ships these from the dyld shared cache, so the files do **not**
/// exist on disk: a naive `Path::exists("/usr/lib/libiconv.2.dylib")` is `false`
/// even though the library loads. An explicit allowlist is the reliable test;
/// the on-disk check in [`system_equivalent`] only adds anything the cache lists
/// as a real file (so a future library outside this set is still handled).
const SYSTEM_LIBS: &[&str] = &[
    "libiconv.2.dylib",
    "libiconv.dylib",
    "libc++.1.dylib",
    "libc++.dylib",
    "libc++abi.dylib",
    "libresolv.9.dylib",
    "libresolv.dylib",
    "libz.1.dylib",
    "libz.dylib",
    "libobjc.A.dylib",
    "libobjc.dylib",
    "libSystem.B.dylib",
    "libcharset.1.dylib",
    "libcompression.dylib",
    "libbz2.1.0.dylib",
    "liblzma.5.dylib",
    "libsqlite3.dylib",
    "libxml2.2.dylib",
    "libcurl.4.dylib",
];

/// The canonical system library path for a `/nix/store` dependency, when the
/// guest is known to ship it. macOS keeps the runtime libraries under `/usr/lib`
/// (served from the dyld shared cache), so a reference to `/usr/lib/<name>`
/// resolves on the guest. Returns `None` when there is no such system library
/// (then the dependency is bundled next to the output instead).
fn system_equivalent(dep: &str) -> Option<String> {
    let name = basename(dep)?;
    let candidate = format!("/usr/lib/{name}");
    // The allowlist is the primary test (the files live in the dyld cache, not
    // on disk); the existence check catches any further `/usr/lib` library the
    // host actually has as a file.
    if SYSTEM_LIBS.contains(&name.as_str()) || Path::new(&candidate).exists() {
        Some(candidate)
    } else {
        None
    }
}

/// The final path component of a dependency reference.
fn basename(dep: &str) -> Option<String> {
    Path::new(dep)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

/// Rewrite a single dependency reference in `binary` from `old` to `new` via
/// `install_name_tool -change`.
fn change_dep(binary: &Path, old: &str, new: &str) -> Result<(), Error> {
    run_checked(
        "install_name_tool",
        Command::new("/usr/bin/install_name_tool")
            .arg("-change")
            .arg(old)
            .arg(new)
            .arg(binary),
    )
}

/// Ad-hoc code-sign (`codesign --force --sign -`) so a rewritten Mach-O has a
/// valid signature again (the kernel kills one whose load commands no longer
/// match its signature).
fn codesign_adhoc(path: &Path) -> Result<(), Error> {
    run_checked(
        "codesign",
        Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(path),
    )
}

/// Run a command, mapping a spawn failure or non-zero exit to a typed error.
fn run_checked(tool: &'static str, command: &mut Command) -> Result<(), Error> {
    let output = command.output().context(SpawnSnafu { tool })?;
    if output.status.success() {
        return Ok(());
    }
    Err(Error::Tool {
        tool,
        status: output.status.to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basename_is_final_component() {
        assert_eq!(basename("/nix/store/abc/lib/libiconv.2.dylib").as_deref(), Some("libiconv.2.dylib"));
        assert_eq!(basename("libfoo.dylib").as_deref(), Some("libfoo.dylib"));
    }

    #[test]
    fn known_runtime_libs_repoint_to_usr_lib() {
        // The listed system runtimes repoint even though they have no on-disk
        // file (they live in the dyld shared cache).
        assert_eq!(
            system_equivalent("/nix/store/x/lib/libiconv.2.dylib").as_deref(),
            Some("/usr/lib/libiconv.2.dylib"),
        );
        assert_eq!(
            system_equivalent("/nix/store/x/lib/libc++.1.dylib").as_deref(),
            Some("/usr/lib/libc++.1.dylib"),
        );
    }

    #[test]
    fn unknown_third_party_lib_is_bundled() {
        // A library the guest does not ship has no system equivalent, so it is
        // bundled (None here).
        assert_eq!(system_equivalent("/nix/store/x/lib/libwgpu_native.dylib"), None);
    }
}
