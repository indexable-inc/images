//! `ix2nix`: read `.ix` source on stdin, write Nix source to stdout.

use std::io::Read as _;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut source = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut source) {
        eprintln!("error: reading stdin: {error}");
        return ExitCode::FAILURE;
    }
    match ix2nix::convert(&source) {
        Ok(nix) => {
            print!("{nix}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
