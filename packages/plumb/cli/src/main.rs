//! plumb: an inspectable bash-subset shell. No arguments starts the REPL;
//! `-c` evaluates a string; a file argument runs a script; `--json` prints
//! the run's full report.

mod repl;

use std::io::{IsTerminal as _, Read as _};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use plumb_core::{Config, Error, Shell};

#[derive(Parser)]
#[command(
    name = "plumb",
    about = "An inspectable bash-subset shell: every run is a value",
    long_about = "An inspectable bash-subset shell. Every run returns a report with per-stage \
                  argv, status, timing, and captured stdout/stderr (including what each pipe \
                  stage fed the next). Runs stay addressable afterwards: ${o[N]}/${e[N]}/${s[N]} \
                  for run N, ${o[N][K]} per pipe stage, negative indexes count from the latest, \
                  and $o/$e/$s alias the last run. Structured paths ${runs[N].stages[K].stdout} \
                  mirror the report JSON field names."
)]
struct Args {
    /// Evaluate this source string and exit.
    #[arg(short = 'c', long = "command", conflicts_with = "script")]
    command: Option<String>,

    /// Print the run's report as JSON instead of streaming output.
    #[arg(long)]
    json: bool,

    /// Script file to run.
    script: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let source = match (&args.command, &args.script) {
        (Some(command), _) => Some(command.clone()),
        (None, Some(path)) => match std::fs::read_to_string(path) {
            Ok(source) => Some(source),
            Err(error) => {
                eprintln!("plumb: {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        },
        (None, None) => {
            if std::io::stdin().is_terminal() {
                None
            } else {
                // `plumb < script` and `... | plumb` run stdin as a script,
                // like bash.
                let mut source = String::new();
                if let Err(error) = std::io::stdin().read_to_string(&mut source) {
                    eprintln!("plumb: stdin: {error}");
                    return ExitCode::FAILURE;
                }
                Some(source)
            }
        }
    };

    let config = Config {
        echo_output: !args.json,
        ..Config::default()
    };
    let shell = match Shell::new(config) {
        Ok(shell) => shell,
        Err(error) => {
            eprintln!("plumb: {error}");
            return ExitCode::FAILURE;
        }
    };

    match source {
        Some(source) => one_shot(&shell, &source, args.json),
        None => repl::run(&shell),
    }
}

fn one_shot(shell: &Shell, source: &str, json: bool) -> ExitCode {
    match shell.eval(source) {
        Ok(report) => {
            if json {
                match serde_json::to_string_pretty(&report) {
                    Ok(rendered) => println!("{rendered}"),
                    Err(error) => {
                        eprintln!("plumb: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            exit_code(report.status)
        }
        Err(Error::ExitRequested { code }) => exit_code(code),
        Err(error) => {
            eprintln!("plumb: {error}");
            ExitCode::from(2)
        }
    }
}

/// Map a wait status onto the 0..=255 process exit range, like a shell.
fn exit_code(status: i32) -> ExitCode {
    // rem_euclid(256) yields 0..=255, always within u8, so this never fails.
    ExitCode::from(u8::try_from(status.rem_euclid(256)).expect("status mod 256 fits in u8"))
}
