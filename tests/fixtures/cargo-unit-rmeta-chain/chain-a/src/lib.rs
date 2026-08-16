//! The leaf of the cutoff chain. Two constraints keep a comment edit here
//! byte-invisible in the compiled artifacts, and the check depends on both:
//!
//! - panic-free: no unwrap/index/overflowing arithmetic anywhere in this
//!   crate, because a reachable panic bakes a file:line string into codegen
//!   and that line number moves with the comment edit;
//! - the workspace pins `debug = 0` for release, because line tables would
//!   legitimately re-bind shifted line numbers into the rlib.
//!
//! The rmeta side (what dependents recompile against) is what the fork's
//! rmeta-stability flags hold byte-identical across the edit.

// CUTOFF-DEMO-EDIT-SITE: the comment-edit variant inserts two comment lines
// directly below this marker, shifting the span of everything that follows.

/// A constant leaf value: no arithmetic, no panic path, nothing for a line
/// shift to move except spans.
pub fn leaf_value() -> u32 {
    37
}
