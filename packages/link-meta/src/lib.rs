//! Embed shell-readable linking metadata in a compiled binary.
//!
//! A binary built with [`stdout_lens!`] carries a small, versioned JSON blob
//! in a dedicated section of the object file:
//!
//! ```json
//! {"v":1,"stdout":{"lens":"json"}}
//! ```
//!
//! The blob names the *lens* — a parsing strategy the shell owns — that
//! applies to the binary's stdout. A lens-aware shell (see the nushell patch
//! in `packages/nushell/patches`) reads the section without executing the
//! binary and parses the command's output into structured data
//! automatically, so `^tool` behaves like `^tool | from json` with no
//! per-tool wrapper.
//!
//! Section names (the canonical contract shared with every consumer):
//!
//! - ELF (Linux, BSDs): [`ELF_SECTION`] (`.ix.link`)
//! - Mach-O (macOS): [`MACHO_SECTION`] (`__DATA,__ix_link`)
//!
//! The schema is versioned so future keys (stderr format, completions, ...)
//! can be added without breaking older readers: a reader accepts `v == 1`
//! and ignores keys it does not understand; a producer bumps `v` only for
//! incompatible changes.
//!
//! [`read::stdout_lens`] is the matching reader, used by the tests here and
//! available to any Rust consumer that wants to honor the declaration.

pub mod read;

/// ELF section holding the metadata blob.
pub const ELF_SECTION: &str = ".ix.link";

/// Mach-O `segment,section` pair holding the metadata blob.
pub const MACHO_SECTION: &str = "__DATA,__ix_link";

/// Declare the lens a shell should apply to this binary's stdout.
///
/// Invoke once, at the crate root of a binary:
///
/// ```
/// link_meta::stdout_lens!("json");
/// ```
///
/// Expands to a `#[used]` static placed in the metadata section, holding
/// `{"v":1,"stdout":{"lens":"json"}}` as raw bytes. On targets with neither
/// ELF nor Mach-O objects (e.g. wasm, Windows) the static is still emitted
/// but not placed in a dedicated section; extending the prototype to PE is
/// future work.
#[macro_export]
macro_rules! stdout_lens {
    ($lens:literal) => {
        #[used]
        #[cfg_attr(
            all(unix, not(target_vendor = "apple")),
            unsafe(link_section = ".ix.link")
        )]
        #[cfg_attr(target_vendor = "apple", unsafe(link_section = "__DATA,__ix_link"))]
        static IX_LINK_META: [u8; {
            $crate::json_for_stdout_lens($lens).len()
        }] = $crate::const_bytes($crate::json_for_stdout_lens($lens));
    };
}

/// The metadata JSON for a stdout lens declaration.
///
/// Exposed for the [`stdout_lens!`] macro expansion; not intended to be
/// called directly.
///
/// # Panics
///
/// At compile time (const evaluation) when `lens` is not a known lens name,
/// so a typo in `stdout_lens!("jsonn")` is a build error, not a silently
/// ignored declaration.
#[must_use]
pub const fn json_for_stdout_lens(lens: &str) -> &str {
    // Only the lens names we ship blobs for; a const lookup keeps the macro
    // free of runtime formatting while guaranteeing the blob is valid JSON.
    match lens.as_bytes() {
        b"json" => r#"{"v":1,"stdout":{"lens":"json"}}"#,
        _ => panic!("link-meta: unknown stdout lens (supported: \"json\")"),
    }
}

/// Copy a `&str` into a fixed-size byte array at compile time.
///
/// Exposed for the [`stdout_lens!`] macro expansion; not intended to be
/// called directly.
///
/// # Panics
///
/// If `N` differs from `s.len()` (cannot happen through the macro, which
/// derives `N` from the same string).
#[must_use]
pub const fn const_bytes<const N: usize>(s: &str) -> [u8; N] {
    let bytes = s.as_bytes();
    assert!(bytes.len() == N, "length mismatch");
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N {
        out[i] = bytes[i];
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    // The test binary itself declares a stdout lens, so reading our own
    // executable exercises the full embed -> link -> read round trip.
    crate::stdout_lens!("json");

    #[test]
    fn json_blob_shape() {
        let blob = crate::json_for_stdout_lens("json");
        let doc: serde_json::Value = serde_json::from_str(blob).expect("blob is valid JSON");
        assert_eq!(doc["v"], 1);
        assert_eq!(doc["stdout"]["lens"], "json");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn round_trip_through_own_executable() {
        let exe = std::env::current_exe().expect("current_exe");
        assert_eq!(
            crate::read::stdout_lens(&exe).as_deref(),
            Some("json"),
            "the lens embedded in this test binary should read back"
        );
    }
}
