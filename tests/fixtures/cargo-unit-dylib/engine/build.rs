//! Builds a native static archive and re-exports its symbol from this crate's
//! dylib.
//!
//! Two build-script channels are under test here. `rustc-link-search` and
//! `rustc-link-lib` have to reach a `dylib` unit's rustc invocation or the link
//! fails outright. `rustc-link-arg` is the subtle one: rustc links a dylib with
//! its own anonymous version script ending `local: *`, which demotes every
//! symbol it did not generate, so `cargo_unit_probe` ends up present in the
//! shared object and unreachable. A second version script naming it is what
//! promotes it back -- ld merges the two and an explicit pattern beats the
//! wildcard. hyperion's `crates/hyperion/build.rs` does exactly this for LMDB.

// A build script talks to cargo over stdout and has no other channel.
#![allow(clippy::print_stdout, reason = "the cargo build-script protocol is stdout")]

use std::{env, fs, path::Path, process::Command};

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let out_dir = env::var("OUT_DIR").expect("cargo always sets OUT_DIR");
    let out = Path::new(&out_dir);

    let source = out.join("probe.c");
    fs::write(&source, "int cargo_unit_probe(void) { return 42; }\n")
        .expect("failed to write the probe source");

    let cc = env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let object = out.join("probe.o");
    let status = Command::new(&cc)
        .args(["-c", "-fPIC", "-o"])
        .arg(&object)
        .arg(&source)
        .status()
        .expect("failed to run the C compiler");
    assert!(status.success(), "{cc} failed to compile the probe");

    let archive = out.join("libcargounitprobe.a");
    let status = Command::new(env::var("AR").unwrap_or_else(|_| "ar".to_string()))
        .arg("rcs")
        .arg(&archive)
        .arg(&object)
        .status()
        .expect("failed to run ar");
    assert!(status.success(), "ar failed to create the probe archive");

    println!("cargo::rustc-link-search=native={out_dir}");
    println!("cargo::rustc-link-lib=static=cargounitprobe");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if matches!(target_os.as_str(), "macos" | "ios") {
        // ld64 unions -exported_symbol with the list rustc generates; no second
        // script exists to write.
        println!("cargo::rustc-link-arg=-Wl,-exported_symbol,_cargo_unit_probe");
        return;
    }

    let script = out.join("probe-exports.map");
    // No `local:` clause: this script adds to rustc's export list, and a
    // `local: *` here would hide every Rust symbol a consumer resolves through.
    fs::write(&script, "{\n  global:\n    cargo_unit_probe;\n};\n")
        .expect("failed to write the version script");
    println!(
        "cargo::rustc-link-arg=-Wl,--version-script={}",
        script.display()
    );
}
