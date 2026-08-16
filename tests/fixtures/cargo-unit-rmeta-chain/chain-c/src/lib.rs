//! Second dependent: one more hop so the convergence assertion covers a
//! transitive dependent, not just the leaf's direct consumer.

pub fn adjusted() -> u32 {
    chain_b::amplified().wrapping_add(3)
}
