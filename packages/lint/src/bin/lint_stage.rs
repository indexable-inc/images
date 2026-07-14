//! `lint-stage <stage>`: one lint stage per invocation, driven by `lint`
//! through the generated dag spec. `--list` prints the stage names for the
//! `stage-list` passthru check that pins the nix-side spec to this binary.

use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use lint::stage::Stage;

/// One lint stage; driven by `lint`.
#[derive(Parser)]
struct Args {
    /// Stage to run.
    #[arg(value_enum, required_unless_present = "list")]
    stage: Option<Stage>,
    /// Print every stage name, one per line (consumed by the nix-side
    /// spec/stage-list consistency check).
    #[arg(long, exclusive = true)]
    list: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.list {
        for variant in Stage::value_variants() {
            if let Some(value) = variant.to_possible_value() {
                println!("{}", value.get_name());
            }
        }
        return ExitCode::SUCCESS;
    }
    let Some(stage) = args.stage else {
        // required_unless_present guarantees this arm is unreachable; keep a
        // loud failure rather than an unwrap.
        eprintln!("lint-stage: specify a stage");
        return ExitCode::FAILURE;
    };
    match lint::stage::run(stage) {
        // A status outside u8 (e.g. signal death surfaced as -1) is still a
        // failure.
        Ok(code) => u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from),
        Err(err) => {
            eprintln!("lint-stage: {err:#}");
            ExitCode::FAILURE
        }
    }
}
