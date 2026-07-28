//! Shared scaffolding for the integration tests: building scratch git
//! repositories to sync from.

// Cargo compiles this module separately into every integration test binary, and
// each of them uses only the part it needs, so an item unused by one is not dead
// code.
#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

/// Run git in `dir`, failing the test with git's own message.
///
/// The environment is pinned so a repository built here does not depend on the
/// machine running the test: no global or system config, and a fixed identity so
/// a commit works on a bare CI sandbox.
pub fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_AUTHOR_NAME", "tree-sync tests")
        .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
        .env("GIT_COMMITTER_NAME", "tree-sync tests")
        .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {} failed in {}: {}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Write a file, creating its parent directories.
pub fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent exists");
    }
    std::fs::write(path, contents).expect("file written");
}

/// A committed repository shaped like the one the rsync bug happened in: a Cargo
/// `target/` at the root that `.gitignore` covers, and a source file called
/// `result.rs` several directories down that it does not.
pub fn sample_repo(root: &Path) {
    write(&root.join(".gitignore"), "/target\n");
    write(&root.join("src/main.rs"), "fn main() {}\n");
    write(
        &root.join("crates/codec/src/impls/result.rs"),
        "pub struct Decoded;\n",
    );
    write(&root.join("crates/codec/src/impls/mod.rs"), "mod result;\n");
    write(&root.join("target/debug/deps/libcodec.rlib"), "binary\n");

    git(root, &["init", "--initial-branch", "main"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "initial"]);
}
