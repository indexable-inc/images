//! Thin CLI over `scipql-core`: index a Rust project with `rust-analyzer scip`,
//! run Soufflé queries over the lowered facts, and apply find/replace edits.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use scipql_core::{Error, OutputRelation, QueryOutput};

#[derive(Parser)]
#[command(name = "scipql", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run `rust-analyzer scip` to produce a SCIP index for a Rust project.
    Index {
        /// Project directory (containing Cargo.toml).
        project: PathBuf,
        /// Where to write the SCIP index.
        #[arg(short, long, default_value = "index.scip")]
        output: PathBuf,
    },
    /// Run a Soufflé program over the index's facts and print every output
    /// relation (TSV, one block per relation). The fact relations
    /// (`occurrence`, `symbol_info`, `document`, `relationship`) are in scope.
    Query {
        /// SCIP index (from `scipql index`).
        index: PathBuf,
        /// Soufflé program file (`.dl`).
        program: PathBuf,
        /// Source root for byte offsets (defaults to the index's project root).
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// Apply a `fix` program's `edit(path, start, end, replacement)` relation.
    /// Prints the unified diff; pass `--write` to update files on disk.
    Fix {
        index: PathBuf,
        /// Soufflé program file that `.output`s the `edit` relation.
        program: PathBuf,
        #[arg(long)]
        root: Option<PathBuf>,
        /// Write the changes instead of printing the diff.
        #[arg(long)]
        write: bool,
    },
    /// Rename every occurrence whose SCIP moniker ends with `selector` to
    /// `new_name`. Dry-run diff unless `--write`.
    Rename {
        index: PathBuf,
        /// Trailing descriptor of the target symbol's moniker (e.g.
        /// `net/Socket#` for a struct, `open().` for a function).
        selector: String,
        /// Replacement identifier.
        new_name: String,
        #[arg(long)]
        root: Option<PathBuf>,
        #[arg(long)]
        write: bool,
    },
}

fn print_relations(output: &QueryOutput) {
    for relation in &output.relations {
        print_relation(relation);
    }
}

fn print_relation(relation: &OutputRelation) {
    println!("# {} ({})", relation.name, relation.columns.join(", "));
    for row in &relation.rows {
        println!("{}", row.join("\t"));
    }
}

fn run(cli: &Cli) -> Result<(), Error> {
    match &cli.command {
        Command::Index { project, output } => scipql_core::index(project, output),
        Command::Query {
            index,
            program,
            root,
        } => {
            let loaded = scipql_core::load_index(index)?;
            let program = read_program(program)?;
            let output = scipql_core::query(&loaded, root.as_deref(), &program)?;
            print_relations(&output);
            Ok(())
        }
        Command::Fix {
            index,
            program,
            root,
            write,
        } => {
            let loaded = scipql_core::load_index(index)?;
            let program = read_program(program)?;
            let diff = scipql_core::fix(&loaded, root.as_deref(), &program, *write)?;
            emit_fix(&diff, *write);
            Ok(())
        }
        Command::Rename {
            index,
            selector,
            new_name,
            root,
            write,
        } => {
            let loaded = scipql_core::load_index(index)?;
            let diff = scipql_core::rename(&loaded, root.as_deref(), selector, new_name, *write)?;
            emit_fix(&diff, *write);
            Ok(())
        }
    }
}

fn read_program(path: &PathBuf) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|source| Error::ReadProgram {
        path: path.clone(),
        source,
    })
}

fn emit_fix(diff: &str, write: bool) {
    print!("{diff}");
    if write {
        eprintln!("scipql: applied edits");
    }
}

fn main() -> ExitCode {
    match run(&Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut message = error.to_string();
            let mut source = std::error::Error::source(&error);
            while let Some(cause) = source {
                message.push_str(": ");
                message.push_str(&cause.to_string());
                source = cause.source();
            }
            eprintln!("scipql: {message}");
            ExitCode::FAILURE
        }
    }
}
