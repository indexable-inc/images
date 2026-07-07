//! `unibind-gen`: render host-language files from the IR embedded in a
//! compiled artifact.
//!
//! The nix glue (`unibind.lib.build`) runs this once per built cdylib, so
//! generated stubs come from the artifact that actually shipped rather than
//! from re-parsing Rust source. Emitted paths (relative to `--out`) print to
//! stdout, one per line, for machine consumption.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _};
use clap::Parser as _;
use unibind_core::ir::Interface;
use unibind_gen::artifact;
use unibind_gen::host::{self, HostEmitter};
use unibind_gen::py::PyEmitter;
use unibind_gen::ts::TsEmitter;

/// Render host-language files (stubs, markers, wrapper modules) from the
/// unibind IR embedded in a compiled artifact.
#[derive(clap::Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// One subcommand per target language. `ex` (phase 5, issue #1995) joins
/// alongside `py` and `ts` with its backend.
#[derive(clap::Subcommand)]
enum Command {
    /// Emit the Python host files: `<package>/<module>.pyi`,
    /// `<package>/py.typed`, and the wrapper `<package>/__init__.py`.
    Py(PyArgs),
    /// Emit the TypeScript host files: `index.d.ts` and the `CommonJS`
    /// `index.js` wrapper around the native addon.
    Ts(TsArgs),
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

#[derive(clap::Args)]
struct TsArgs {
    /// Compiled cdylib (or renamed `.node` addon) carrying the embedded IR.
    #[arg(long)]
    artifact: PathBuf,

    /// Basename of the native addon: the generated `index.js` loads
    /// `./native/<addon>.node`, so packaging must place the cdylib there.
    #[arg(long)]
    addon: String,

    /// Output root; files are written at paths relative to it.
    #[arg(long)]
    out: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Py(args) => run_py(&args),
        Command::Ts(args) => run_ts(&args),
    }
}

fn run_py(args: &PyArgs) -> anyhow::Result<()> {
    let embedded = artifact::read(&args.artifact)?;
    let interface = single_interface(&args.artifact, &embedded, "py")?;

    let emitter = PyEmitter {
        package: args.package.clone(),
        skip_init: args.skip_init,
    };
    emit_and_write(&emitter, interface, &args.out)
}

fn run_ts(args: &TsArgs) -> anyhow::Result<()> {
    let embedded = artifact::read(&args.artifact)?;
    let interface = single_interface(&args.artifact, &embedded, "ts")?;

    let emitter = TsEmitter {
        addon: args.addon.clone(),
    };
    emit_and_write(&emitter, interface, &args.out)
}

/// The one interface of `artifact_path`; every generator handles exactly
/// one exported module per addon.
fn single_interface<'a>(
    artifact_path: &Path,
    embedded: &'a artifact::EmbeddedInterfaces,
    target: &str,
) -> anyhow::Result<&'a Interface> {
    match embedded.interfaces.as_slice() {
        [interface] => Ok(interface),
        [] => bail!("{} embeds no unibind interface", artifact_path.display()),
        several => bail!(
            "{} embeds {} unibind interfaces ({}); the {target} generator handles exactly \
             one per artifact",
            artifact_path.display(),
            several.len(),
            several
                .iter()
                .map(|interface| interface.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn emit_and_write(
    emitter: &dyn HostEmitter,
    interface: &Interface,
    out: &Path,
) -> anyhow::Result<()> {
    let files = emitter
        .emit(interface)
        .with_context(|| format!("emitting the {} host files", emitter.target()))?;
    host::write_host_files(out, &files)?;

    for file in &files {
        println!("{}", file.path);
    }
    Ok(())
}
