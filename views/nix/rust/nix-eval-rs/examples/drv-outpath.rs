//! Recompute every store derivation's output paths from its own bytes and
//! compare them to the paths cppnix wrote inside it.
//!
//! This is the check `hello.outPath` is one instance of, run against every
//! derivation a real store happens to hold. It exercises the whole step-1
//! pipeline at once -- the ATerm writer, output masking, the
//! `hashDerivationModulo` recursion through input derivations, `compressHash`
//! and the base-32 encoding -- against an oracle cppnix maintains itself:
//! `Derivation::checkInvariants` (`src/libstore/derivations.cc:1398`) asserts
//! exactly this equality, so every `.drv` in the store is a case that already
//! passed it once.
//!
//! It is a different property from `drv-roundtrip`, which is why it is a
//! different program. The round trip says the bytes survive a parse; this says
//! the bytes *mean* what cppnix thinks they mean. Either can be green while
//! the other is red, and a single verdict over both would hide which.
//!
//!     drv-outpath [--store /nix/store] <file.drv>...
//!     drv-outpath [--store /nix/store] < list-of-paths
//!
//! Every outcome is counted and named. `not-input-addressed` and `deferred`
//! are **not** agreements: a corpus of nothing but fixed-output derivations
//! would report zero mismatches while proving nothing about `makeOutputPath`,
//! so the summary carries them separately and the pass condition is stated in
//! terms of `agrees` having a nonzero count of its own.

use nix_eval_rs::drv;
use nix_eval_rs::drvpath::{self, DrvHash, DrvSource, PathCheck};
use std::cell::RefCell;
use std::collections::BTreeMap;

/// Reads input derivations off the filesystem.
///
/// Deliberately holds no parsed derivations. Memoisation happens one level up,
/// on the `DrvHash` rather than on the parse: `hash_derivation_modulo` checks
/// its memo before it asks for bytes, so each input is read at most once
/// anyway, and a second cache of every parsed `Derivation` in a 400,000-file
/// store would cost gigabytes to save the handful of re-reads where a
/// derivation is both an input and a top-level argument.
struct StoreFs {
    reads: RefCell<usize>,
}

impl DrvSource for StoreFs {
    fn read_drv(&self, drv_path: &str) -> Result<(drv::Derivation, String), String> {
        let text = std::fs::read_to_string(drv_path).map_err(|e| e.to_string())?;
        *self.reads.borrow_mut() += 1;
        let parsed = drv::parse(&text).map_err(|e| e.to_string())?;
        let name = drvpath::name_from_drv_path(drv_path)
            .ok_or_else(|| format!("'{drv_path}' is not a store derivation path"))?
            .to_owned();
        Ok((parsed, name))
    }
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut store_dir = "/nix/store".to_owned();
    let mut paths: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--store" {
            store_dir = args.next().ok_or("--store needs a directory")?;
        } else {
            paths.push(arg);
        }
    }
    if paths.is_empty() {
        use std::io::BufRead as _;
        for line in std::io::stdin().lock().lines() {
            let line = line?;
            if !line.trim().is_empty() {
                paths.push(line);
            }
        }
    }
    if paths.is_empty() {
        return Err("usage: drv-outpath [--store DIR] <file.drv>... (or paths on stdin)".into());
    }

    let source = StoreFs {
        reads: RefCell::new(0),
    };
    let mut memo: BTreeMap<String, DrvHash> = BTreeMap::new();

    let mut rebuilt_agrees = 0usize;
    let mut rebuilt_differs = 0usize;
    let mut rebuilt_skipped = 0usize;
    let mut drv_path_agrees = 0usize;
    let mut drv_path_differs = 0usize;
    let mut agrees = 0usize;
    let mut outputs_agreed = 0usize;
    let mut differs = 0usize;
    let mut not_input_addressed = 0usize;
    let mut deferred = 0usize;
    let mut errors = 0usize;

    for path in &paths {
        let Some(name) = drvpath::name_from_drv_path(path) else {
            errors += 1;
            println!("{path}\terror\tnot a store derivation path, so its name is unknown");
            continue;
        };
        let original = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                errors += 1;
                println!("{path}\terror\tunreadable: {e}");
                continue;
            }
        };
        let parsed = match drv::parse(&original) {
            Ok(parsed) => parsed,
            Err(e) => {
                errors += 1;
                println!("{path}\terror\t{e}");
                continue;
            }
        };
        // Where the `.drv` itself lands, which is a different computation
        // from where its outputs do and can be wrong on its own.
        let computed_drv_path = drvpath::derivation_store_path(&store_dir, &parsed, name);
        if computed_drv_path == *path {
            drv_path_agrees += 1;
        } else {
            drv_path_differs += 1;
            println!("{path}\tdrv-path-differs\tcomputed {computed_drv_path}");
        }
        // Take the derivation apart and build it back up the way
        // `derivationStrict` will have to, then require the bytes and the
        // `.drv` path to match. This is the construction direction, which no
        // amount of parsing checks: the builder gets no disk order and no
        // output paths, it has to produce both.
        match drvpath::inputs_of(&parsed, name) {
            None => rebuilt_skipped += 1,
            Some(inputs) => {
                match drvpath::build_input_addressed(&store_dir, &inputs, &source, &mut memo) {
                    Err(e) => {
                        rebuilt_differs += 1;
                        println!("{path}\trebuild-error\t{e}");
                    }
                    Ok(built) => {
                        if built.aterm == original && built.drv_path == *path {
                            rebuilt_agrees += 1;
                        } else {
                            rebuilt_differs += 1;
                            let what = if built.drv_path != *path {
                                format!("drv path {}", built.drv_path)
                            } else {
                                "bytes".to_owned()
                            };
                            println!("{path}\trebuild-differs\t{what}");
                        }
                    }
                }
            }
        }
        match drvpath::check_output_paths(&store_dir, &parsed, name, &source, &mut memo) {
            Ok(PathCheck::Agrees { outputs }) => {
                agrees += 1;
                outputs_agreed += outputs;
                println!("{path}\tagrees\t{outputs} output(s)");
            }
            Ok(PathCheck::Differs { output, want, got }) => {
                differs += 1;
                println!("{path}\tdiffers\toutput '{output}': computed {want}, file says {got}");
            }
            Ok(PathCheck::NotInputAddressed) => {
                not_input_addressed += 1;
                println!(
                    "{path}\tnot-input-addressed\tfixed, floating or impure: cppnix computes no path here either"
                );
            }
            Ok(PathCheck::Deferred) => {
                deferred += 1;
                println!(
                    "{path}\tdeferred\tan input is floating or impure, so no path can exist yet"
                );
            }
            Err(e) => {
                errors += 1;
                println!("{path}\terror\t{e}");
            }
        }
    }

    println!(
        "RESULT drv-outpath store={store_dir} files={} agrees={agrees} outputs-agreed={outputs_agreed} differs={differs} not-input-addressed={not_input_addressed} deferred={deferred} errors={errors} input-drvs-read={}",
        paths.len(),
        source.reads.borrow(),
    );
    println!(
        "RESULT drv-selfpath store={store_dir} files={} agrees={drv_path_agrees} differs={drv_path_differs}",
        paths.len(),
    );
    println!(
        "RESULT drv-rebuild store={store_dir} files={} agrees={rebuilt_agrees} differs={rebuilt_differs} not-input-addressed={rebuilt_skipped}",
        paths.len(),
    );
    // `differs == 0` on its own is satisfied by a run that compared nothing,
    // which is the failure mode this whole harness exists to avoid, so the
    // pass condition names a positive count as well.
    if differs > 0 || errors > 0 || drv_path_differs > 0 || rebuilt_differs > 0 {
        Err("output paths do not agree".into())
    } else if agrees == 0 {
        Err("nothing was input-addressed, so no output path was compared".into())
    } else {
        Ok(())
    }
}
