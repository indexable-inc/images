use std::path::PathBuf;

#[test]
fn test_no_forgotten_test_files() {
    let test_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests");
    testutils::assert_no_forgotten_test_files(&test_dir);
}

#[cfg(feature = "nfs")]
mod test_nfs;
mod test_overlay;
mod test_snapshot;
