use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Kind {
    Type1,
    Type2,
    Type3 { similarity: f64 },
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
    pub enable_sequences: bool,
    pub sequence_window_size: usize,
}

/// Default Jaccard similarity threshold for Type-3 clone detection.
const DEFAULT_TYPE3_THRESHOLD: f64 = 0.7;

impl Default for DetectConfig {
    fn default() -> Self {
        Self {
            enable_type3: false,
            type3_threshold: DEFAULT_TYPE3_THRESHOLD,
            enable_sequences: false,
            sequence_window_size: crate::sequences::DEFAULT_WINDOW_SIZE,
        }
    }
}
