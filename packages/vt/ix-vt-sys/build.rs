//! Link `ix-vt-sys` against the Nix-built `libghostty-vt`.
//!
//! The bindings are checked in (`src/bindings.rs`), so this build script only
//! arranges the link. The library directory is supplied out of band by the Nix
//! derivation that builds `libghostty-vt`, via the `IX_VT_GHOSTTY_LIB_DIR`
//! environment variable, because the C library is not on the system linker
//! path and is not published to a registry.
//!
//! We link the self-contained dynamic library (`-l dylib=ghostty-vt`) rather
//! than the static archive. The static `libghostty-vt.a` does not bundle its
//! C++ dependencies (`libhighway`, `libsimdutf`, `libutfcpp`) and would force
//! every consumer to also pass those archives plus `-lc++`; the dylib carries
//! them, so one link directive is enough.
//!
//! A build-script `rustc-link-search` does not propagate to the final binary
//! link in the repo's per-unit Nix build, so the workspace also adds the same
//! directory to every unit's rustc `-L` flags. Emitting it here keeps a plain
//! `cargo build` working when `IX_VT_GHOSTTY_LIB_DIR` is set.

use std::env;

/// Environment variable that names the directory holding `libghostty-vt`.
/// The owning Nix derivation sets this; the swap PR that wires `ix-vt` into
/// `packages/tui` will reuse the same variable.
const LIB_DIR_ENV: &str = "IX_VT_GHOSTTY_LIB_DIR";

fn main() {
    println!("cargo:rerun-if-env-changed={LIB_DIR_ENV}");

    if let Ok(lib_dir) = env::var(LIB_DIR_ENV)
        && !lib_dir.is_empty()
    {
        println!("cargo:rustc-link-search=native={lib_dir}");
    }

    // Link the self-contained shared library. The matching `-L` search path is
    // provided either by the env var above or by the workspace's per-unit
    // rustc flags.
    println!("cargo:rustc-link-lib=dylib=ghostty-vt");
}
