//! Golden pairs under `tests/golden/`: each `<name>.ix` must convert
//! byte-for-byte to its `<name>.golden` sibling (Nix source) and must emit its
//! `<name>.schema.golden` sibling (a JSON Schema document). Both carry the
//! `.golden` extension so the repo-wide formatters and lint gates stay off
//! these exact renderer bytes -- `.json` in particular is rejected outright
//! for repository-owned files.
//!
//! Both outputs for every input, deliberately. They come from one pass over
//! one parse, so pinning them side by side is what shows a reader that an
//! annotation's eval-time check and its published schema say the same thing --
//! and it means an untyped fixture pins the empty-root schema too, which is
//! the case a schema-only fixture set would never cover.

use std::path::PathBuf;

/// Every `.ix` fixture in the directory. Pinned so a rename or a lost sibling
/// shows up as a failure rather than as a harness that matched nothing.
const FIXTURES: usize = 14;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[test]
fn every_golden_pair_converts_exactly() {
    let mut checked = 0;
    let entries = std::fs::read_dir(golden_dir()).expect("golden dir exists");
    for entry in entries {
        let path = entry.expect("golden dir entry").path();
        if path.extension().is_none_or(|extension| extension != "ix") {
            continue;
        }

        let source = std::fs::read_to_string(&path).expect("golden .ix readable");

        let expected_path = path.with_extension("golden");
        let expected = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|_| panic!("{} has no .golden sibling", path.display()));
        let converted = ix2nix::convert(&source)
            .unwrap_or_else(|error| panic!("{} failed to convert:\n{error}", path.display()));
        assert_eq!(converted, expected, "{} diverged", path.display());

        let schema_path = path.with_extension("schema.golden");
        let expected_schema = std::fs::read_to_string(&schema_path)
            .unwrap_or_else(|_| panic!("{} has no .schema.golden sibling", path.display()));
        let schema = ix2nix::schema(&source)
            .unwrap_or_else(|error| panic!("{} has no schema:\n{error}", path.display()));
        assert_eq!(schema, expected_schema, "{} diverged", schema_path.display());

        checked += 1;
    }

    assert_eq!(
        checked, FIXTURES,
        "expected {FIXTURES} golden fixtures; update FIXTURES when adding one"
    );
}
