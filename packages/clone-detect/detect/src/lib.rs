mod detector;
pub mod jaccard;
pub mod lsh;
pub mod sequences;
mod type3;
mod types;

pub use detector::instances;
pub use type3::compute_similarity;
pub use types::{
    ByteRange, CloneGroup, DetectConfig, DetectionResult, DetectionStats, Fragment, Kind, LineRange,
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
