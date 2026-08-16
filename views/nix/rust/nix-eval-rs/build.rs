//! Fingerprints this crate's source into the binary.
//!
//! A cached compiled `Module` is only valid against the compiler that produced
//! it. What "the compiler" covers -- the whole of `src`, the non-Rust files
//! the binary embeds, the resolved dependency versions and the feature set --
//! is in `compiler-fingerprint.rs` beside the code that computes it.

include!("compiler-fingerprint.rs");

/// The features cargo enabled for this build, as the suffixes of the
/// `CARGO_FEATURE_*` variables it sets (uppercased, `-` as `_`).
///
/// Read from the environment rather than from `cfg!`, which a build script
/// cannot use to see its own crate's features, and kept in the build script
/// rather than the shared file because the variables exist only here.
fn enabled_features() -> Vec<String> {
    let mut features: Vec<String> = std::env::vars()
        .filter_map(|(key, _)| key.strip_prefix("CARGO_FEATURE_").map(str::to_owned))
        .collect();
    features.sort();
    features
}

/// A build script that cannot fingerprint the crate must fail the build, not
/// emit a constant: a cache keyed on a fingerprint that does not describe the
/// compiler is worse than no cache. Returning an error rather than panicking
/// keeps the workspace's `panic` denial intact; cargo prints it and fails.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=compiler-fingerprint.rs");
    // `src` as a directory, so a *new* file in it is a change; the per-file
    // lines below are what catch an edit to one that already exists.
    println!("cargo::rerun-if-changed=src");

    let crate_root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR")?);
    let features = enabled_features();
    let inputs = compiler_fingerprint_inputs(&crate_root, &features)?;
    // Derived from the hashed set rather than listed again beside it: a file
    // that is in the hash and not watched is a stale stamp, and keeping one
    // list means that cannot be arranged by editing only one of them.
    for input in &inputs {
        if let Some(path) = &input.path {
            println!("cargo::rerun-if-changed={}", path.display());
        }
    }

    let hex = hash_compiler_fingerprint_inputs(&inputs)?;
    println!("cargo::rustc-env=IXE_COMPILER_FINGERPRINT={hex}");
    // Stamped so the tests can recompute the fingerprint with the feature set
    // this build actually had; `the_stamped_feature_list_is_this_builds_own`
    // is what stops that from being the build script agreeing with itself.
    println!(
        "cargo::rustc-env=IXE_COMPILER_FEATURES={}",
        features.join(",")
    );
    Ok(())
}
