//! Golden pairs under `tests/golden/`: each `<name>.ix` must convert
//! byte-for-byte to its `<name>.golden` sibling (Nix source; the `.golden`
//! extension keeps the repo-wide Nix formatter and lint gates off these
//! exact renderer bytes).

use std::path::PathBuf;

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
        checked += 1;
    }

    // Guard against the harness silently matching nothing (e.g. after a
    // directory rename).
    assert!(checked >= 11, "expected at least 11 golden pairs, saw {checked}");
}
