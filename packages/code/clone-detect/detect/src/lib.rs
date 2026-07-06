mod detector;
pub mod jaccard;
pub mod lsh;
pub mod sequences;
mod type3;
mod types;

pub use detector::instances;
pub use type3::{compute_similarity, compute_similarity_with};
pub use types::{
    ByteRange, CloneGroup, DetectConfig, DetectionResult, DetectionStats, Fragment, Kind,
    LineRange, Type3Metric,
};

#[cfg(test)]
mod tests {
    mod config;
    mod detection;
    mod helpers;
    mod multilang;
    mod pipeline;
    mod serialization;
    mod similarity;
    mod stats;
    mod structural;
    mod type3;
}
