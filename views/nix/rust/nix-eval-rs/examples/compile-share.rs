//! Measures what fraction of a run is spent compiling rather than evaluating,
//! which is the denominator any compilation-cache claim needs.
//!
//! One file per invocation is the supported shape: the corpus contains
//! expressions this evaluator does not terminate on, and an in-process loop
//! over all of them has no way to bound one. The caller runs this under
//! `timeout` and an address-space cap and aggregates the lines.
//!
//! Usage: compile-share <reps> <base-dir> <file>...
//! Emits one TSV line per file: name, compile ns, eval ns, status.

use nix_eval_rs::compile;
use nix_eval_rs::eval::drive;
use nix_eval_rs::host::RealFs;
use nix_eval_rs::value2::Value;
use nix_eval_rs::vm::Vm;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut args = std::env::args().skip(1);
    let reps: u32 = args
        .next()
        .ok_or("usage: compile-share <reps> <base-dir> <file>...")?
        .parse()?;
    let base = args
        .next()
        .ok_or("usage: compile-share <reps> <base-dir> <file>...")?;

    for path in args {
        let Ok(src) = std::fs::read_to_string(&path) else {
            println!("{path}\t0\t0\tunreadable");
            continue;
        };
        let mut compile_time = Duration::ZERO;
        let mut eval_time = Duration::ZERO;
        let mut status = "ok";

        for _ in 0..reps {
            let start = Instant::now();
            let module = compile::compile_source(
                &src,
                &base,
                compile::Origin::File(&path),
                &nix_eval_rs::eval::Settings::current(),
            );
            compile_time += start.elapsed();
            let module = match module {
                Ok(module) => Rc::new(module),
                Err(_) => {
                    status = "compile-failed";
                    break;
                }
            };
            let start = Instant::now();
            let outcome = {
                let mut vm = Vm::from_process_settings();
                vm.start_module(&module);
                drive(&mut vm, &RealFs).and_then(|value| {
                    vm.start_print(value);
                    drive(&mut vm, &RealFs)
                })
            };
            eval_time += start.elapsed();
            if !matches!(outcome, Ok(Value::Str(_))) {
                status = "eval-failed";
                break;
            }
        }
        println!(
            "{path}\t{}\t{}\t{status}",
            compile_time.as_nanos(),
            eval_time.as_nanos()
        );
    }
    Ok(())
}
