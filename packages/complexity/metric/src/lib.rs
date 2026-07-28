//! Per-function complexity over tree-sitter ASTs.
//!
//! Cognitive complexity is the headline metric because it is the only
//! code-only metric with a published validation against human-rated
//! understandability (Munoz Baron, Wyrich and Wagner, ESEM 2020). That
//! validation is narrow: a replication found it no better than the older
//! metrics it replaces (Lavazza et al., JSS 2023), and an fMRI study found
//! `McCabe` cyclomatic complexity has no correlation at all with comprehension
//! time or correctness (Peitek et al., ICSE 2021). Treat the output as
//! triage, not as a measurement of difficulty.

pub mod kinds;
mod measure;

pub use measure::{Unit, measure};

#[cfg(test)]
mod dump;
#[cfg(test)]
mod tests;
