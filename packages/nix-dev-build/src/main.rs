//! Incremental builds of a nix source checkout.
//!
//! `nix build .#nix-ix` recompiles the whole modular C++ closure in a sandbox
//! for a one-line change. The tree is a meson project, so ninja recompiles only
//! what changed. This tool is that loop as one command: configure the build
//! directory if it is absent, build, then name the binary it produced.
//!
//! Distinct from `nix-ninja-build-nix`, the other incremental lane here. That
//! one materializes the packaged source into a scratch directory and turns each
//! compilation unit into its own content-addressed derivation, and it runs on
//! x86_64-linux only. This one builds the checkout you are editing, with plain
//! local ninja, on any system the dev shell evaluates for.

mod build;
mod checkout;
mod identity;
mod shell;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::Parser;
use serde::Serialize;

use crate::build::State;
use crate::checkout::Checkout;
use crate::identity::{Ambient, Built};

/// The binary nix development actually iterates on, and its path under the
/// build directory. Ninja target names are paths, so one string serves as both.
const DEFAULT_TARGET: &str = "src/nix/nix";

/// Build a nix source checkout incrementally with meson and ninja.
#[derive(Parser, Debug, PartialEq, Eq)]
#[command(version, about)]
struct Cli {
    /// Checkout to build. Defaults to the nearest one at or above the working
    /// directory.
    #[arg(long, value_name = "PATH")]
    checkout: Option<PathBuf>,

    /// Meson build directory, taken relative to the checkout.
    #[arg(long, default_value = "build", value_name = "DIR")]
    build_dir: PathBuf,

    /// Ninja target to build.
    #[arg(long, default_value = DEFAULT_TARGET, value_name = "TARGET")]
    target: String,

    /// Dev shell to build inside, as a `devShells` attribute of the checkout's
    /// flake. `native-ccacheStdenv` reuses compiler output across
    /// reconfigures; `native-clangStdenv` swaps the compiler.
    #[arg(long, default_value = "default", value_name = "ATTR")]
    shell: String,

    /// Discard the build directory's contents and configure it again, keeping
    /// the options meson recorded.
    #[arg(long)]
    reconfigure: bool,

    /// Emit one JSON document on stdout; build progress goes to stderr.
    #[arg(long)]
    json: bool,

    /// Extra arguments for ninja, after `--`.
    #[arg(last = true, value_name = "NINJA_ARG")]
    ninja_args: Vec<String>,
}

impl Cli {
    /// This invocation as an argument vector, with the checkout pinned to its
    /// resolved path so the run inside the dev shell does not depend on the
    /// working directory.
    fn to_args(&self, checkout: &Path) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "--checkout".into(),
            checkout.into(),
            "--build-dir".into(),
            self.build_dir.clone().into(),
            "--target".into(),
            self.target.clone().into(),
            "--shell".into(),
            self.shell.clone().into(),
        ];
        if self.reconfigure {
            args.push("--reconfigure".into());
        }
        if self.json {
            args.push("--json".into());
        }
        if !self.ninja_args.is_empty() {
            args.push("--".into());
            args.extend(self.ninja_args.iter().map(OsString::from));
        }
        args
    }
}

#[derive(Serialize)]
struct Report {
    checkout: PathBuf,
    revision: Option<RevisionReport>,
    build_dir: PathBuf,
    target: String,
    configure_seconds: Option<f64>,
    build_seconds: f64,
    built: Option<Built>,
    ambient_nix: Option<Ambient>,
}

#[derive(Serialize)]
struct RevisionReport {
    short: String,
    dirty: bool,
}

/// Refusals here are instructions the reader is meant to act on ("pass
/// --checkout"), so they are rendered as one message rather than propagated out
/// of `main`, which would bury them under a backtrace wherever `RUST_BACKTRACE`
/// is set.
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("nix-dev-build: {}", error_chain::format(error.as_ref()));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let checkout = Checkout::find(cli.checkout.clone())?;

    if !shell::inside() {
        shell::reenter(&checkout.root, &cli.shell, &cli.to_args(&checkout.root))?;
    }

    let build_dir = if cli.build_dir.is_absolute() {
        cli.build_dir.clone()
    } else {
        checkout.root.join(&cli.build_dir)
    };

    let configure = configure_if_needed(&checkout.root, &build_dir, cli.reconfigure, cli.json)?;
    let elapsed = build::ninja(&build_dir, &cli.target, &cli.ninja_args, cli.json)?;

    let built = Built::find(&build_dir, &cli.target);
    let ambient = built.as_ref().and_then(Ambient::find);
    let report = Report {
        checkout: checkout.root.clone(),
        revision: checkout.revision().map(|revision| RevisionReport {
            short: revision.short,
            dirty: revision.dirty,
        }),
        build_dir,
        target: cli.target.clone(),
        configure_seconds: configure.as_ref().map(Duration::as_secs_f64),
        build_seconds: elapsed.as_secs_f64(),
        built,
        ambient_nix: ambient,
    };

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        report.print_human();
    }
    Ok(())
}

/// Configure the build directory when it is absent or when asked, and refuse to
/// build through one that will not do what the caller expects.
fn configure_if_needed(
    checkout: &Path,
    build_dir: &Path,
    reconfigure: bool,
    quiet: bool,
) -> Result<Option<Duration>> {
    let state = build::state(build_dir, checkout)?;
    if matches!(state, State::Absent) {
        // --wipe requires an existing directory, so the first configure is
        // plain whether or not --reconfigure was passed.
        return build::setup(checkout, build_dir, false, quiet).map(Some);
    }
    if reconfigure {
        return build::setup(checkout, build_dir, true, quiet).map(Some);
    }
    match state {
        State::Ready | State::Absent => Ok(None),
        State::ForeignSource(other) => bail!(
            "{} is configured for a different checkout\n  \
             configured for: {}\n  \
             asked to build: {}\n  \
             rerun with --reconfigure, or pass --build-dir",
            build_dir.display(),
            other.display(),
            checkout.display(),
        ),
        State::Unusable(why) => bail!("{why}\n  rerun with --reconfigure"),
    }
}

impl Report {
    fn print_human(&self) {
        if let Some(seconds) = self.configure_seconds {
            println!("configured {} in {seconds:.1}s", self.build_dir.display());
        }
        println!("built {} in {:.1}s", self.target, self.build_seconds);
        println!();
        match &self.built {
            Some(built) => {
                println!("  binary   {}", built.path.display());
                if let Some(version) = &built.version {
                    println!("  version  {version}");
                }
            }
            None => println!("  target   {} (no binary at that path)", self.target),
        }
        match &self.revision {
            Some(revision) if revision.dirty => {
                println!("  tree     {} plus uncommitted changes", revision.short);
            }
            Some(revision) => println!("  tree     {}", revision.short),
            None => println!("  tree     not a git checkout"),
        }
        if let Some(ambient) = &self.ambient_nix
            && ambient.same_version_string
        {
            println!();
            println!(
                "  {} reports that same version string, so --version cannot tell\n  \
                 the two apart. Run the path above to use what you just built.",
                ambient.path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field has to survive the trip through `nix develop`, and a new
    /// flag that `to_args` forgets would silently revert to its default in the
    /// run that does the work.
    #[test]
    fn arguments_round_trip_through_the_dev_shell() {
        let original = Cli {
            checkout: Some(PathBuf::from("/checkout")),
            build_dir: PathBuf::from("elsewhere"),
            target: "src/libutil-tests/libutil-tests".to_owned(),
            shell: "native-ccacheStdenv".to_owned(),
            reconfigure: true,
            json: true,
            ninja_args: vec!["-j".to_owned(), "4".to_owned()],
        };
        let mut argv: Vec<OsString> = vec!["nix-dev-build".into()];
        argv.extend(original.to_args(Path::new("/checkout")));
        assert_eq!(Cli::parse_from(argv), original);
    }

    #[test]
    fn defaults_round_trip_too() {
        let original = Cli::parse_from(["nix-dev-build"]);
        let mut argv: Vec<OsString> = vec!["nix-dev-build".into()];
        argv.extend(original.to_args(Path::new("/checkout")));
        let inner = Cli::parse_from(argv);
        assert_eq!(inner.checkout, Some(PathBuf::from("/checkout")));
        assert_eq!(inner.target, original.target);
        assert_eq!(inner.build_dir, original.build_dir);
        assert_eq!(inner.shell, original.shell);
        assert!(!inner.reconfigure);
        assert!(!inner.json);
        assert!(inner.ninja_args.is_empty());
    }
}
