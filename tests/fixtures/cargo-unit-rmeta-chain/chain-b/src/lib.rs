//! First dependent: compiles against chain-a's rmeta (and, under
//! embed-metadata=no, its thin rlib extern pair). Wrapping arithmetic keeps
//! this crate panic-free too, so its own artifact bytes are insensitive to
//! line shifts upstream.

pub fn amplified() -> u32 {
    chain_a::leaf_value().wrapping_mul(3)
}
