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
/// reference survives. Returns the staged path (`output`).
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

    // Collect every /nix/store dependency, then plan each one. A dependency may
    // legitimately appear more than once in `otool -L` for an unusual binary;
    // dedupe so we issue one rewrite per distinct path.
    let deps = nix_store_deps(output)?;
    let mut bundled: BTreeSet<String> = BTreeSet::new();
    for dep in &deps {
        if let Some(system) = system_equivalent(dep) {
            change_dep(output, dep, &system)?;
        } else {
            let name = basename(dep).ok_or_else(|| Error::DepNoBasename { dep: dep.clone() })?;
            // Copy the dylib next to the output (once per distinct basename) and
            // re-sign it, since it now lives at a new path.
            if bundled.insert(name.clone()) {
                let bundled_path = out_dir.join(&name);
                copy_writable(Path::new(dep), &bundled_path)?;
                codesign_adhoc(&bundled_path)?;
            }
            change_dep(output, dep, &format!("@loader_path/{name}"))?;
        }
    }

    // The load commands changed, so the prior signature is invalid; re-sign.
    codesign_adhoc(output)?;

    // No silent fallback: a surviving /nix/store reference means the staged
    // binary would not start on a guest, so fail loudly with the offenders.
    let remaining = nix_store_deps(output)?;
    if !remaining.is_empty() {
        return Err(Error::StorePathsRemain {
            path: output.to_path_buf(),
            remaining: remaining.join("\n"),
        });
    }
    Ok(output.to_path_buf())
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
