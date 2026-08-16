//! `unibind-gen`: render host-language files from the IR embedded in a
//! compiled artifact.
//!
//! The nix glue (`unibind.lib.build`) runs this once per built cdylib, so
//! generated stubs come from the artifact that actually shipped rather than
//! from re-parsing Rust source. Emitted paths (relative to `--out`) print to
//! stdout, one per line, for machine consumption.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use clap::Parser as _;
use unibind_core::ir::Interface;
use unibind_gen::artifact;
use unibind_gen::ex::ExEmitter;
use unibind_gen::host::{self, HostEmitter};
use unibind_gen::jvm::JvmEmitter;
use unibind_gen::py::PyEmitter;
use unibind_gen::ts::TsEmitter;
use unibind_gen::wasm::WasmEmitter;

/// Render host-language files (stubs, markers, wrapper modules) from the
/// unibind IR embedded in a compiled artifact.
#[derive(clap::Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// One subcommand per target language.
#[derive(clap::Subcommand)]
enum Command {
    /// Emit the Python host files: `<package>/<module>.pyi`,
    /// `<package>/py.typed`, and the wrapper `<package>/__init__.py`.
    Py(PyArgs),
    /// Emit the TypeScript host files: `index.d.ts` and the `CommonJS`
    /// `index.js` wrapper around the native addon.
    Ts(TsArgs),
    /// Emit the Elixir host files: `lib/<app>/native.ex` with the NIF
    /// stubs and the typespec'd `lib/<app>.ex` wrapper.
    Ex(ExArgs),
    /// Emit the Java host file: a single `<Class>.java` wrapping the
    /// C-ABI symbols through the FFM API.
    Jvm(JvmArgs),
    /// Emit the browser host files: `index.d.ts`, `schemas.ts`, and the ESM
    /// `index.js` wrapper around the `wasm-bindgen` module.
    Wasm(WasmArgs),
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

#[derive(clap::Args)]
struct WasmArgs {
    /// Compiled `wasm32-unknown-unknown` cdylib carrying the embedded IR.
    #[arg(long)]
    artifact: PathBuf,

    /// Module specifier of the `wasm-bindgen --target web` JavaScript output
    /// (`./wasm/ix_sdk.js`): the generated `index.js` imports every compiled
    /// export and the initializer from it.
    #[arg(long)]
    module: String,

    /// Output root; files are written at paths relative to it.
    #[arg(long)]
    out: PathBuf,
}

#[derive(clap::Args)]
struct ExArgs {
    /// Compiled NIF library carrying the embedded IR; its file name is the
    /// soname the generated loader strips the extension from.
    #[arg(long)]
    artifact: PathBuf,

    /// Output root; files are written at paths relative to it.
    #[arg(long)]
    out: PathBuf,
}

#[derive(clap::Args)]
struct JvmArgs {
    /// Compiled cdylib carrying the embedded IR.
    #[arg(long)]
    artifact: PathBuf,

    /// Java package the class is declared in (`com.example.sample`); the
    /// file lands under the matching directory tree. Omit for the unnamed
    /// package at the output root.
    #[arg(long)]
    package: Option<String>,

    /// Output root; files are written at paths relative to it.
    #[arg(long)]
    out: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Py(args) => run_py(&args),
        Command::Ts(args) => run_ts(&args),
        Command::Ex(args) => run_ex(&args),
        Command::Jvm(args) => run_jvm(&args),
        Command::Wasm(args) => run_wasm(&args),
    }
}

/// Read the artifact's one interface and emit it: the whole run for every
/// target whose emitter needs nothing else from the artifact path.
fn run_host(artifact: &Path, out: &Path, emitter: &dyn HostEmitter) -> anyhow::Result<()> {
    let embedded = artifact::read(artifact)?;
    let interface = single_interface(artifact, &embedded, emitter.target())?;
    emit_and_write(emitter, interface, out)
}

fn run_py(args: &PyArgs) -> anyhow::Result<()> {
    let emitter = PyEmitter {
        package: args.package.clone(),
        skip_init: args.skip_init,
    };
    run_host(&args.artifact, &args.out, &emitter)
}

fn run_ts(args: &TsArgs) -> anyhow::Result<()> {
    let emitter = TsEmitter {
        addon: args.addon.clone(),
    };
    run_host(&args.artifact, &args.out, &emitter)
}

fn run_wasm(args: &WasmArgs) -> anyhow::Result<()> {
    let emitter = WasmEmitter {
        module: args.module.clone(),
    };
    run_host(&args.artifact, &args.out, &emitter)
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

fn run_jvm(args: &JvmArgs) -> anyhow::Result<()> {
    let emitter = JvmEmitter {
        package: args.package.clone(),
    };
    run_host(&args.artifact, &args.out, &emitter)
}

fn run_ex(args: &ExArgs) -> anyhow::Result<()> {
    let Some(nif_soname) = args.artifact.file_name() else {
        bail!(
            "{} has no file name to derive the NIF soname from",
            args.artifact.display()
        );
    };
    let emitter = ExEmitter {
        nif_soname: nif_soname.to_string_lossy().into_owned(),
    };
    run_host(&args.artifact, &args.out, &emitter)
}
