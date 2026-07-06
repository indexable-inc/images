//! `unibind-gen`: render host-language files from the IR embedded in a
//! compiled artifact.
//!
//! The nix glue (`unibind.lib.build`) runs this once per built cdylib, so
//! generated stubs come from the artifact that actually shipped rather than
//! from re-parsing Rust source. Emitted paths (relative to `--out`) print to
//! stdout, one per line, for machine consumption.

use std::path::PathBuf;

use anyhow::{bail, Context as _};
use clap::Parser as _;
use unibind_gen::artifact;
use unibind_gen::host::{self, HostEmitter as _};
use unibind_gen::py::PyEmitter;

/// Render host-language files (stubs, markers, wrapper modules) from the
/// unibind IR embedded in a compiled artifact.
#[derive(clap::Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// One subcommand per target language. `ts` (phase 3, issue #1993) and `ex`
/// (phase 5, issue #1995) join alongside `py` with their backends.
#[derive(clap::Subcommand)]
enum Command {
    /// Emit the Python host files: `<package>/<module>.pyi`,
    /// `<package>/py.typed`, and the wrapper `<package>/__init__.py`.
    Py(PyArgs),
}

#[derive(clap::Args)]
struct PyArgs {
    /// Compiled cdylib (or any object file) carrying the embedded IR.
    #[arg(long)]
    artifact: PathBuf,

    /// Import-package name the files land under (e.g. `scipql`).
    #[arg(long)]
    package: String,

    /// Output root; files are written at paths relative to it.
    #[arg(long)]
    out: PathBuf,

    /// Skip the wrapper `__init__.py` (the caller ships a hand-written one).
    #[arg(long)]
    skip_init: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Py(args) => run_py(&args),
    }
}

fn run_py(args: &PyArgs) -> anyhow::Result<()> {
    let embedded = artifact::read(&args.artifact)?;
    let interface = match embedded.interfaces.as_slice() {
        [interface] => interface,
        [] => bail!("{} embeds no unibind interface", args.artifact.display()),
        several => bail!(
            "{} embeds {} unibind interfaces ({}); the py generator handles exactly one \
             per artifact",
            args.artifact.display(),
            several.len(),
            several
                .iter()
                .map(|interface| interface.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };

    let emitter = PyEmitter {
        package: args.package.clone(),
        skip_init: args.skip_init,
    };
    let files = emitter
        .emit(interface)
        .with_context(|| format!("emitting the {} host files", emitter.target()))?;
    host::write_host_files(&args.out, &files)?;

    for file in &files {
        println!("{}", file.path);
    }
    Ok(())
}
