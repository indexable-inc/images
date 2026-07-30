//! `ix2nix`: read `.ix` source on stdin, write Nix source to stdout.
//!
//! With `--schema`, write the module's JSON Schema instead of its Nix. Both
//! come from the same annotations, so the schema is a view of what the emitted
//! Nix already checks.

use std::io::Read as _;
use std::process::ExitCode;

const USAGE: &str = "usage: ix2nix [--schema] < module.ix";

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let render: fn(&str) -> Result<String, ix2nix::Error> = match arguments.next().as_deref() {
        None => ix2nix::convert,
        Some("--schema") => ix2nix::schema,
        Some(unknown) => {
            eprintln!("error: unknown argument `{unknown}`\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    if arguments.next().is_some() {
        eprintln!("error: too many arguments\n{USAGE}");
        return ExitCode::FAILURE;
    }

    let mut source = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut source) {
        eprintln!("error: reading stdin: {error}");
        return ExitCode::FAILURE;
    }
    match render(&source) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
