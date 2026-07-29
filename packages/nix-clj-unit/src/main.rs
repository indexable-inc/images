//! `nix-clj-unit` renders a Clojure source tree as a dependency graph of
//! namespaces, one node per namespace, so that Nix can AOT-compile each one in
//! its own derivation. It is the Clojure sibling of `nix-cargo-unit` (one
//! derivation per rustc invocation) and `nix-kbuild-unit` (one per C
//! translation unit).
//!
//! # Why the graph has to exist
//!
//! A unit compiles with a classpath holding the dependency jars, the *compiled
//! output* of the namespaces it requires, a source root containing only its own
//! `.clj`, and its own output directory:
//!
//! ```text
//! java -cp <dep jars>:<dependency unit outputs>:<srcroot with only this ns>:$out \
//!   clojure.main -e "(binding [*compile-path* \"$out\"] (compile 'the.namespace))"
//! ```
//!
//! The load-bearing condition is that a unit's classpath must **not** contain
//! its dependencies' `.clj` source, only their compiled output. Clojure prefers
//! a `.class` over the `.clj` it came from only when the class file is
//! *strictly newer*, and Nix normalises every store mtime to 1. So a dependency
//! whose source is visible ties on mtime, loses the comparison, and gets
//! recompiled from source -- transitively, in every unit that can see it, which
//! collapses the graph back into one big build. Knowing each namespace's exact
//! require set is what lets the Nix side hand a unit its dependencies'
//! *outputs* and nothing else.

mod graph;
mod model;
mod naming;
mod ns_form;
mod reader;

use std::path::PathBuf;

use clap::Parser as _;
use color_eyre::eyre::WrapErr as _;

#[derive(Debug, clap::Parser)]
#[command(
    version,
    about = "Render a Clojure source tree as a per-namespace dependency graph"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Walk source roots and write the namespace graph as JSON.
    Render(RenderArgs),
}

#[derive(Debug, clap::Args)]
struct RenderArgs {
    /// Clojure source root to walk, repeatable. Paths in the output keep the
    /// root exactly as written here, so pass the path the Nix side expects to
    /// see.
    #[arg(long = "src", value_name = "PATH", required = true)]
    src: Vec<PathBuf>,

    /// Where to write the graph JSON.
    #[arg(long, value_name = "PATH")]
    out: PathBuf,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    match Cli::parse().command {
        Command::Render(args) => {
            let rendered = graph::render(&args.src)?;
            let mut json = serde_json::to_string_pretty(&rendered)
                .wrap_err("serializing the namespace graph")?;
            json.push('\n');
            std::fs::write(&args.out, json)
                .wrap_err_with(|| format!("writing {}", args.out.display()))?;
        }
    }

    Ok(())
}
