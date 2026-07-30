//! Staleness: whether a memory's `based_on:` files still hash to what they did
//! when it was last validated.
//!
//! A stale memory is reported, never hidden. Reading a stale memory unflagged
//! is the harm; ranking it low would only hide the flag.

use crate::{
    error::{self, Result},
    model::{BASED_ON_HASH_HEX_CHARS, BasedOn, Memory},
};
use snafu::{OptionExt, ResultExt};
use std::path::Path;

/// Whether a memory's evidence still matches, and the sentence naming what
/// moved when it does not.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Staleness {
    pub stale: bool,
    pub reason: Option<String>,
}

/// Check every `based_on` entry of one memory.
///
/// # Errors
///
/// Returns [`crate::Error::ReadFile`] when a `based_on` file exists but cannot
/// be read, and [`crate::Error::BadGlob`] when a glob pattern is malformed. A
/// path that does not exist is a staleness reason, not an error.
pub fn check(memory: &Memory) -> Result<Staleness> {
    let mut reasons = Vec::new();

    for entry in &memory.based_on {
        // A glob stands for a set of files, so there is no single content to
        // hash and nothing to compare; `memory-based-on-missing` still checks
        // that it matches something.
        if entry.is_glob() {
            continue;
        }

        let target = memory.root.join(&entry.path);
        let Some(computed) = hash_file(&target)? else {
            reasons.push(format!("based_on moved: {}", entry.path));
            continue;
        };
        if let Some(recorded) = &entry.blake3
            && !hashes_match(recorded, &computed)
        {
            reasons.push(format!("based_on changed: {}", entry.path));
        }
    }

    Ok(if reasons.is_empty() {
        Staleness {
            stale: false,
            reason: None,
        }
    } else {
        Staleness {
            stale: true,
            reason: Some(reasons.join("; ")),
        }
    })
}

/// Hash of one file, truncated to [`BASED_ON_HASH_HEX_CHARS`], or `None` when
/// the file does not exist.
///
/// # Errors
///
/// Returns [`crate::Error::ReadFile`] for every IO failure other than "not
/// found": an unreadable file is a real problem and must not read as a moved
/// one.
pub fn hash_file(path: &Path) -> Result<Option<String>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(io_error) if io_error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(io_error) => {
            return Err(io_error).context(error::ReadFileSnafu { path });
        }
    };
    let mut hex = blake3::hash(&bytes).to_hex().to_string();
    hex.truncate(BASED_ON_HASH_HEX_CHARS);
    Ok(Some(hex))
}

/// Whether a `based_on` entry resolves to at least one existing file. A glob
/// resolves when it matches something.
///
/// # Errors
///
/// Returns [`crate::Error::BadGlob`] for a malformed pattern,
/// [`crate::Error::GlobWalk`] when expansion fails, and
/// [`crate::Error::NonUtf8Path`] when the root path cannot be spelled as a
/// glob.
pub fn resolves(root: &Path, entry: &BasedOn) -> Result<bool> {
    let target = root.join(&entry.path);
    if !entry.is_glob() {
        return Ok(target.exists());
    }

    let pattern = target
        .to_str()
        .context(error::NonUtf8PathSnafu { path: &target })?;
    let mut matches = glob::glob(pattern).context(error::BadGlobSnafu { pattern })?;
    match matches.next() {
        None => Ok(false),
        Some(Ok(_)) => Ok(true),
        Some(Err(glob_error)) => Err(glob_error).context(error::GlobWalkSnafu { pattern }),
    }
}

/// Compare a recorded hash with a computed one over their common prefix, so a
/// value shortened by hand still validates. Both are lowercase hex, checked at
/// parse time, so slicing on a byte index is slicing on a character.
fn hashes_match(recorded: &str, computed: &str) -> bool {
    let width = recorded.len().min(computed.len());
    width > 0 && recorded[..width] == computed[..width]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::parse_memory;

    struct Fixture {
        _dir: tempfile::TempDir,
        root: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("a temp dir");
            let root = dir.path().to_path_buf();
            std::fs::create_dir_all(root.join(".memories")).expect("a memories dir");
            std::fs::create_dir_all(root.join("src")).expect("a source dir");
            Self { _dir: dir, root }
        }

        fn write(&self, relative: &str, contents: &str) {
            std::fs::write(self.root.join(relative), contents).expect("writing a fixture file");
        }

        fn memory(&self, based_on: &str) -> Memory {
            let contents = format!("---\ntldr: A line\nbased_on:\n{based_on}---\nBody.\n");
            let path = self.root.join(".memories/a-slug.md");
            parse_memory(&path, &self.root, &contents).expect("fixture must parse")
        }
    }

    #[test]
    fn matching_hash_is_not_stale() {
        let fixture = Fixture::new();
        fixture.write("src/rank.rs", "fn main() {}\n");
        let hash = hash_file(&fixture.root.join("src/rank.rs"))
            .expect("hashing a file that exists")
            .expect("the file exists");
        let memory = fixture.memory(&format!("  - path: src/rank.rs\n    blake3: {hash}\n"));
        assert_eq!(check(&memory).expect("checking"), Staleness::default());
    }

    #[test]
    fn changed_content_is_stale_and_names_the_path() {
        let fixture = Fixture::new();
        fixture.write("src/rank.rs", "fn main() {}\n");
        let hash = hash_file(&fixture.root.join("src/rank.rs"))
            .expect("hashing")
            .expect("the file exists");
        let memory = fixture.memory(&format!("  - path: src/rank.rs\n    blake3: {hash}\n"));
        fixture.write("src/rank.rs", "fn main() { edited() }\n");

        let staleness = check(&memory).expect("checking");
        assert!(staleness.stale, "an edited file makes the memory stale");
        let reason = staleness.reason.expect("a stale memory has a reason");
        assert!(
            reason.contains("src/rank.rs"),
            "the reason must name the path, got {reason}"
        );
    }

    #[test]
    fn a_shortened_recorded_hash_still_validates() {
        // The format's example writes a truncated digest, so comparison is over
        // the common prefix rather than the whole string.
        let fixture = Fixture::new();
        fixture.write("src/rank.rs", "fn main() {}\n");
        let full = hash_file(&fixture.root.join("src/rank.rs"))
            .expect("hashing")
            .expect("the file exists");
        let short: String = full.chars().take(10).collect();
        let memory = fixture.memory(&format!("  - path: src/rank.rs\n    blake3: {short}\n"));
        assert!(
            !check(&memory).expect("checking").stale,
            "a 10-character prefix of the same digest is not a mismatch"
        );
    }

    #[test]
    fn a_missing_based_on_path_is_stale_even_without_a_recorded_hash() {
        let fixture = Fixture::new();
        let memory = fixture.memory("  - path: src/gone.rs\n");
        let staleness = check(&memory).expect("checking");
        assert!(staleness.stale);
        assert_eq!(
            staleness.reason.as_deref(),
            Some("based_on moved: src/gone.rs")
        );
    }

    #[test]
    fn a_glob_is_never_hashed_but_must_still_match_something() {
        let fixture = Fixture::new();
        fixture.write("src/rank.rs", "fn main() {}\n");
        let memory = fixture.memory("  - path: src/*.rs\n");
        assert!(
            !check(&memory).expect("checking").stale,
            "a glob carries no hash, so it cannot be stale"
        );
        assert!(
            resolves(&fixture.root, &memory.based_on[0]).expect("expanding the glob"),
            "src/*.rs matches src/rank.rs"
        );

        let unmatched = fixture.memory("  - path: docs/*.md\n");
        assert!(
            !resolves(&fixture.root, &unmatched.based_on[0]).expect("expanding the glob"),
            "a glob matching nothing does not resolve"
        );
    }

    #[test]
    fn several_stale_entries_are_all_named() {
        let fixture = Fixture::new();
        let memory = fixture.memory("  - path: src/one.rs\n  - path: src/two.rs\n");
        let reason = check(&memory)
            .expect("checking")
            .reason
            .expect("two moved paths");
        assert!(
            reason.contains("src/one.rs") && reason.contains("src/two.rs"),
            "{reason}"
        );
    }
}
