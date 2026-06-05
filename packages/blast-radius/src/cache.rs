//! On-disk cache of a base revision's `.#checks.x86_64-linux` evaluation, keyed
//! by commit SHA.
//!
//! Evaluating the full check catalog is the tool's dominant cost (~11 min for
//! ix's ~4300 checks), and it is paid twice on every PR run: once for base, once
//! for head. But `base` is the merge-base on `main`: an immutable commit whose
//! evaluation is fully determined by its tree and `flake.lock`, both pinned in
//! the commit. So the base eval is cached by SHA and the common PR cases
//! (re-pushing a branch, sibling PRs off the same main tip) reuse it instead of
//! re-evaluating from scratch.
//!
//! Fail-safe by construction: any I/O, parse, or version mismatch is reported on
//! stderr and treated as a miss (read) or no-op (write). A broken, stale, or
//! absent cache can only fall back to a full fresh eval, never produce a wrong
//! report: the cached value is just the attribute -> `.drv` map the diff
//! compares, and `.drv` paths are input-addressed, so a cached path for a fixed
//! SHA is exactly what a fresh eval would produce.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::nix::{Check, EvalFailure, EvalResult};

/// Bump when the on-disk shape changes; entries written by a different version
/// are ignored (treated as a miss) rather than misread.
const CACHE_VERSION: u32 = 1;

/// Serialize side: borrows the live eval so storing needs no clone.
#[derive(Serialize)]
struct StoredRef<'a> {
    version: u32,
    checks: &'a [Check],
    failures: &'a [EvalFailure],
}

/// Deserialize side.
#[derive(Deserialize)]
struct Stored {
    version: u32,
    checks: Vec<Check>,
    failures: Vec<EvalFailure>,
}

/// The directory holding per-SHA cache files. Honors `IX_BLAST_RADIUS_CACHE_DIR`,
/// then `XDG_CACHE_HOME`, then `$HOME/.cache`; on the CI runner `$HOME`
/// (`/var/lib/ix-ci-runner`) persists across jobs, so the cache survives between
/// runs. Returns `None` when no base directory can be resolved (caching off).
fn cache_dir() -> Option<PathBuf> {
    let from_env = |key: &str| {
        std::env::var(key)
            .ok()
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = from_env("IX_BLAST_RADIUS_CACHE_DIR") {
        return Some(dir);
    }
    if let Some(dir) = from_env("XDG_CACHE_HOME") {
        return Some(dir.join("blast-radius"));
    }
    from_env("HOME").map(|home| home.join(".cache").join("blast-radius"))
}

fn entry_path(sha: &str) -> Option<PathBuf> {
    cache_dir().map(|dir| dir.join(format!("{sha}.json")))
}

/// Decode a cache payload, rejecting a version mismatch. `Err` carries a reason
/// for the caller to log; pure so the round-trip is unit tested without I/O.
fn decode(raw: &str) -> std::result::Result<EvalResult, String> {
    let stored: Stored = serde_json::from_str(raw).map_err(|err| format!("unparsable: {err}"))?;
    if stored.version != CACHE_VERSION {
        return Err(format!("version {} != {CACHE_VERSION}", stored.version));
    }
    Ok(EvalResult {
        checks: stored.checks,
        failures: stored.failures,
    })
}

/// Encode an eval into a versioned cache payload.
fn encode(result: &EvalResult) -> serde_json::Result<String> {
    serde_json::to_string(&StoredRef {
        version: CACHE_VERSION,
        checks: &result.checks,
        failures: &result.failures,
    })
}

/// Load a cached base eval for `sha`, or `None` on any miss/error.
pub fn load(sha: &str) -> Option<EvalResult> {
    let path = entry_path(sha)?;
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) => {
            // A missing file is the ordinary miss; only note other errors.
            if err.kind() != std::io::ErrorKind::NotFound {
                eprintln!("blast-radius: cache read {} failed: {err}", path.display());
            }
            return None;
        }
    };
    match decode(&raw) {
        Ok(result) => {
            eprintln!(
                "blast-radius: base eval cache hit for {sha} ({} checks)",
                result.checks.len()
            );
            Some(result)
        }
        Err(reason) => {
            eprintln!("blast-radius: ignoring cache {}: {reason}", path.display());
            None
        }
    }
}

/// Persist a base eval for `sha`. Best-effort: failures are reported and ignored.
/// Writes a temp file then renames so a concurrent reader (a sibling PR run
/// evaluating the same base) never sees a half-written entry.
pub fn store(sha: &str, result: &EvalResult) {
    let Some(dir) = cache_dir() else {
        return;
    };
    if let Err(err) = fs::create_dir_all(&dir) {
        eprintln!("blast-radius: cache mkdir {} failed: {err}", dir.display());
        return;
    }
    let path = dir.join(format!("{sha}.json"));
    let tmp = dir.join(format!(".{sha}.{}.tmp", std::process::id()));
    let json = match encode(result) {
        Ok(json) => json,
        Err(err) => {
            eprintln!("blast-radius: cache encode for {sha} failed: {err}");
            return;
        }
    };
    if let Err(err) = fs::write(&tmp, json) {
        eprintln!("blast-radius: cache write {} failed: {err}", tmp.display());
        return;
    }
    if let Err(err) = fs::rename(&tmp, &path) {
        eprintln!("blast-radius: cache rename to {} failed: {err}", path.display());
        if let Err(cleanup) = fs::remove_file(&tmp) {
            eprintln!("blast-radius: cache cleanup {} failed: {cleanup}", tmp.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CACHE_VERSION, decode, encode};
    use crate::nix::{Check, EvalFailure, EvalResult};

    fn sample() -> EvalResult {
        EvalResult {
            checks: vec![
                Check {
                    attr: "rust-test-foo".to_owned(),
                    drv_path: "/nix/store/aaa-foo.drv".to_owned(),
                },
                Check {
                    attr: "image-bar".to_owned(),
                    drv_path: "/nix/store/bbb-bar.drv".to_owned(),
                },
            ],
            failures: vec![EvalFailure {
                attr: "unfree-allowlist".to_owned(),
                error: "unfree allowlist mismatch".to_owned(),
            }],
        }
    }

    // A round-trip preserves every check and failure: the cached base eval the
    // diff reads back is byte-for-byte the eval that was stored.
    #[test]
    fn encode_decode_round_trips() {
        let original = sample();
        let raw = encode(&original).expect("encode");
        let restored = decode(&raw).expect("decode");
        assert_eq!(restored, original);
    }

    // A payload written by a different on-disk version is rejected (a miss), not
    // misread into a wrong report.
    #[test]
    fn decode_rejects_version_mismatch() {
        let raw = format!(
            r#"{{"version":{},"checks":[],"failures":[]}}"#,
            CACHE_VERSION + 1
        );
        assert!(decode(&raw).is_err());
    }

    // Garbage on disk is a miss, never a panic.
    #[test]
    fn decode_rejects_garbage() {
        assert!(decode("not json").is_err());
        assert!(decode("{}").is_err());
    }
}
