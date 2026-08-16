//! Check the ATerm reader and writer against real `.drv` files: they must come
//! back byte-identical, they must already be in the order a *constructed*
//! derivation would have to produce, and the run must say which output shapes
//! it actually covered.
//!
//! Three checks rather than one, because the round trip alone is weaker than
//! it looks and the ladder quoted it as though it were the whole story:
//!
//! * **round trip** -- `parse` then `unparse` reproduces the file's bytes.
//!   Anchored on bytes cppnix wrote, so it is not a self-consistency test, but
//!   it can only fail on a bug that survives the composition.
//! * **canonical order** -- the on-disk order equals `drv::canonicalise`'s.
//!   The round trip is *structurally* blind to this: parsing keeps whatever
//!   order it found and the writer emits it back, so a wrong ordering rule
//!   passes every file. `derivationStrict` has no disk order to inherit, so
//!   this is the check that speaks to how a derivation gets built.
//! * **shape census** -- how many files carried each of the five output kinds,
//!   multiple outputs, structured attributes, non-ASCII bytes, escapes. A
//!   count with no census cannot distinguish "420,831 derivations agreed" from
//!   "420,831 near-identical derivations agreed"; the summary carries both.
//!
//! ```text
//! drv-roundtrip <file.drv>...
//! drv-roundtrip < list-of-paths
//! ```
//!
//! Paths come from the command line, or one per line on stdin when none are
//! given -- a whole store does not fit in an argument list, and splitting the
//! run with `xargs` would emit one summary per batch, which is exactly the
//! partial-count-read-as-total failure the summary line exists to prevent.
//!
//! Prints one TSV line per file (`path <TAB> ok|differs|unordered|error <TAB>
//! detail`) and two final summary lines carrying their denominators.

use nix_eval_rs::drv::{self, OutputKind};

/// How many files carried each shape. Every counter is a count of *files*, so
/// they share one denominator and can be read against the total directly.
#[derive(Default)]
struct Census {
    input_addressed: usize,
    ca_fixed: usize,
    ca_floating: usize,
    deferred: usize,
    impure: usize,
    unrecognised: usize,
    multi_output: usize,
    structured_attrs: usize,
    non_ascii: usize,
    escaped: usize,
    no_inputs: usize,
}

impl Census {
    fn add(&mut self, parsed: &drv::Derivation, original: &str) {
        for output in &parsed.outputs {
            match output.kind() {
                OutputKind::InputAddressed => self.input_addressed += 1,
                OutputKind::CaFixed => self.ca_fixed += 1,
                OutputKind::CaFloating => self.ca_floating += 1,
                OutputKind::Deferred => self.deferred += 1,
                OutputKind::Impure => self.impure += 1,
                OutputKind::Unrecognised => self.unrecognised += 1,
            }
        }
        if parsed.outputs.len() > 1 {
            self.multi_output += 1;
        }
        if parsed.env.iter().any(|e| e.name == "__json") {
            self.structured_attrs += 1;
        }
        if !original.is_ascii() {
            self.non_ascii += 1;
        }
        if original.contains('\\') {
            self.escaped += 1;
        }
        if parsed.input_drvs.is_empty() && parsed.input_srcs.is_empty() {
            self.no_inputs += 1;
        }
    }
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let mut paths: Vec<String> = std::env::args().skip(1).collect();
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
        return Err("usage: drv-roundtrip <file.drv>... (or paths on stdin)".into());
    }

    let mut ok = 0usize;
    let mut differs = 0usize;
    let mut unordered = 0usize;
    let mut errors = 0usize;
    let mut dynamic = 0usize;
    let mut census = Census::default();

    for path in &paths {
        let original = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) => {
                errors += 1;
                println!("{path}\terror\tunreadable: {e}");
                continue;
            }
        };
        match drv::parse(&original) {
            Err(drv::DrvError::DynamicDerivations) => {
                dynamic += 1;
                println!("{path}\tdynamic\trefused by name, not mis-parsed");
            }
            Err(e) => {
                errors += 1;
                println!("{path}\terror\t{e}");
            }
            Ok(parsed) => {
                census.add(&parsed, &original);
                // Order first: a file that fails both should be reported as
                // the ordering fault, since a wrong ordering rule is the
                // finding and a byte difference is how it would show up.
                if !drv::is_canonical(&parsed) {
                    unordered += 1;
                    let mut sorted = parsed.clone();
                    drv::canonicalise(&mut sorted);
                    let field = if sorted.outputs != parsed.outputs {
                        "outputs"
                    } else if sorted.input_drvs != parsed.input_drvs {
                        "inputDrvs"
                    } else if sorted.input_srcs != parsed.input_srcs {
                        "inputSrcs"
                    } else {
                        "env"
                    };
                    println!(
                        "{path}\tunordered\tcppnix wrote {field} in an order canonicalise does not produce"
                    );
                    continue;
                }
                let again = drv::unparse(&parsed, false);
                if again == original {
                    ok += 1;
                    println!("{path}\tok\t{} bytes", original.len());
                } else {
                    differs += 1;
                    // Name the first differing byte: over a file this size a
                    // diff is unreadable and the offset is the whole answer.
                    let at = again
                        .as_bytes()
                        .iter()
                        .zip(original.as_bytes())
                        .position(|(a, b)| a != b)
                        .unwrap_or(original.len().min(again.len()));
                    let window = |s: &str| {
                        s.get(at.saturating_sub(20)..(at + 20).min(s.len()))
                            .unwrap_or("")
                            .replace(['\n', '\t'], " ")
                    };
                    println!(
                        "{path}\tdiffers\tfirst at byte {at}: want {:?} got {:?}",
                        window(&original),
                        window(&again)
                    );
                }
            }
        }
    }

    println!(
        "RESULT drv-roundtrip files={} ok={ok} differs={differs} unordered={unordered} dynamic={dynamic} errors={errors}",
        paths.len()
    );
    println!(
        "RESULT drv-census files={} outputs-input-addressed={} outputs-ca-fixed={} outputs-ca-floating={} outputs-deferred={} outputs-impure={} outputs-unrecognised={} files-multi-output={} files-structured-attrs={} files-non-ascii={} files-with-escapes={} files-no-inputs={}",
        paths.len(),
        census.input_addressed,
        census.ca_fixed,
        census.ca_floating,
        census.deferred,
        census.impure,
        census.unrecognised,
        census.multi_output,
        census.structured_attrs,
        census.non_ascii,
        census.escaped,
        census.no_inputs,
    );
    if differs > 0 || errors > 0 || unordered > 0 {
        Err("round-trip not clean".into())
    } else {
        Ok(())
    }
}
