use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Similarity metric used to confirm a Type-3 candidate pair.
///
/// Both operate on the same sorted multiset of structural subtree features; they
/// differ only in the denominator, which changes what "similar" means:
///
/// - [`Jaccard`](Type3Metric::Jaccard): `|A ∩ B| / |A ∪ B|`. Symmetric, but
///   penalizes size differences, so a clone with a few inserted/deleted
///   statements drops below threshold even when one fragment is nearly a subset
///   of the other.
/// - [`Overlap`](Type3Metric::Overlap): `|A ∩ B| / min(|A|, |B|)` (overlap
///   coefficient / containment). Does not penalize the size gap, so it catches
///   the dominant "copy-paste then edit" case (Sherlock N-overlap, IEEE TC
///   2019). The flip side: generic structural boilerplate contains easily, so
///   at the same threshold overlap reports far more groups (measured 40x on
///   this repo at 0.7). Use it for recall-oriented sweeps, preferably with a
///   higher threshold (>= 0.8); pure insert/delete clones score near 1.0 under
///   it, so a high threshold costs little recall on the cases it exists for.
///
/// Jaccard is the default deliberately: it keeps default output precise (and
/// byte-compatible with the tool's history), while overlap is the opt-in
/// wide-net mode. Each Type-3 group reports the metric that produced it
/// ([`Kind::Type3`]), so `similarity` values are never ambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Type3Metric {
    #[default]
    Jaccard,
    Overlap,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Kind {
    Type1,
    Type2,
    /// A near-miss (gapped) clone. `similarity` is the score under `metric`, so
    /// the two fields must be read together: a `0.8` under `overlap` and under
    /// `jaccard` mean different things.
    Type3 { similarity: f64, metric: Type3Metric },
    Sequence { statements: usize },
}

/// Byte offset range within a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

/// Line number range within a source file (1-indexed, inclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fragment {
    pub file: PathBuf,
    pub byte_range: ByteRange,
    pub lines: LineRange,
    pub kind: String,
}

impl Fragment {
    /// Construct a fragment from a scanned file and one of its nodes.
    #[must_use]
    pub fn from_node(file: &clone_scanner::File, node: &clone_hash::NodeInfo) -> Self {
        Self {
            file: file.path.clone(),
            byte_range: ByteRange {
                start: node.byte_range.start,
                end: node.byte_range.end,
            },
            lines: LineRange {
                start: node.start_line,
                end: node.end_line,
            },
            kind: node.kind.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloneGroup {
    pub clone_type: Kind,
    pub fragments: Vec<Fragment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectionResult {
    pub instances: Vec<CloneGroup>,
    pub stats: DetectionStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetectionStats {
    pub files_scanned: usize,
    pub nodes_analyzed: usize,
    pub total_lines: usize,
    pub duplicated_lines: usize,
    pub duplication_pct: f64,
    pub type1_groups: usize,
    pub type2_groups: usize,
    pub type3_groups: usize,
    pub sequence_groups: usize,
}

#[derive(Debug, Clone)]
pub struct DetectConfig {
    pub enable_type3: bool,
    pub type3_threshold: f64,
    pub type3_metric: Type3Metric,
    pub enable_sequences: bool,
    pub sequence_window_size: usize,
}

/// Default similarity threshold for Type-3 clone detection.
const DEFAULT_TYPE3_THRESHOLD: f64 = 0.7;

impl Default for DetectConfig {
    fn default() -> Self {
        Self {
            enable_type3: false,
            type3_threshold: DEFAULT_TYPE3_THRESHOLD,
            type3_metric: Type3Metric::default(),
            enable_sequences: false,
            sequence_window_size: crate::sequences::DEFAULT_WINDOW_SIZE,
        }
    }
}
