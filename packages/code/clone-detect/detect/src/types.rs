use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

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
///   higher threshold (>= 0.9 at repo scale); pure insert/delete clones score
///   near 1.0 under it, so a high threshold costs little recall on the cases
///   it exists for.
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
    Type3 {
        similarity: f64,
        metric: Type3Metric,
    },
    Sequence {
        statements: usize,
    },
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
    #[serde(default)]
    pub generated: bool,
}

impl Fragment {
    /// Construct a fragment from a scanned file and one of its nodes.
    #[must_use]
    pub fn from_node(
        file: &clone_scanner::File,
        node: &clone_hash::NodeInfo,
        generated: bool,
    ) -> Self {
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
            generated,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CloneGroup {
    pub clone_type: Kind,
    pub fragments: Vec<Fragment>,
}

impl Serialize for CloneGroup {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Output<'a> {
            clone_type: Kind,
            impact_lines: usize,
            generated: bool,
            fragments: &'a [Fragment],
        }

        Output {
            clone_type: self.clone_type,
            impact_lines: self.line_impact(),
            generated: self.is_generated(),
            fragments: &self.fragments,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for CloneGroup {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Input {
            clone_type: Kind,
            #[serde(default)]
            impact_lines: Option<usize>,
            #[serde(default)]
            generated: Option<bool>,
            fragments: Vec<Fragment>,
        }

        let input = Input::deserialize(deserializer)?;
        let group = Self {
            clone_type: input.clone_type,
            fragments: input.fragments,
        };
        if input.impact_lines.is_some_and(|impact| impact != group.line_impact()) {
            return Err(serde::de::Error::custom("impact_lines does not match fragments"));
        }
        if input.generated.is_some_and(|generated| generated != group.is_generated()) {
            return Err(serde::de::Error::custom("generated does not match fragments"));
        }
        Ok(group)
    }
}

impl CloneGroup {
    /// Estimated lines removable by consolidating this group into its largest
    /// fragment. This is a ranking signal, while the global statistic still
    /// deduplicates overlapping line ranges across every group.
    #[must_use]
    pub fn line_impact(&self) -> usize {
        let total: usize = self.fragments.iter().map(fragment_line_count).sum();
        let original = self
            .fragments
            .iter()
            .map(fragment_line_count)
            .max()
            .unwrap_or_default();
        total.saturating_sub(original)
    }

    /// True when every fragment is generated output. Mixed authored/generated
    /// groups stay actionable and therefore rank with authored code.
    #[must_use]
    pub fn is_generated(&self) -> bool {
        !self.fragments.is_empty() && self.fragments.iter().all(|fragment| fragment.generated)
    }
}

pub(crate) fn file_is_generated(file: &clone_scanner::File) -> bool {
    let snapshot_path = file
        .path
        .components()
        .any(|component| component.as_os_str() == "snapshots");
    snapshot_path || generated_header(&file.source)
}

pub(crate) fn generated_header(source: &str) -> bool {
    const MAX_HEADER_BYTES: usize = 4096;
    const MARKERS: [&[u8]; 4] = [
        b"automatically generated",
        b"generated by",
        b"do not edit",
        b"@generated",
    ];

    let prefix = &source.as_bytes()[..source.len().min(MAX_HEADER_BYTES)];
    let end = prefix
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(7)
        .map_or(prefix.len(), |(index, _)| index);
    let header = &prefix[..end];

    MARKERS.iter().any(|marker| {
        header
            .windows(marker.len())
            .any(|window| window.eq_ignore_ascii_case(marker))
    })
}

fn fragment_line_count(fragment: &Fragment) -> usize {
    fragment
        .lines
        .end
        .saturating_sub(fragment.lines.start)
        .saturating_add(1)
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
#[derive(Debug, Clone, Serialize, Deserialize)]
>>>>>>>
<<<<<<<
            fragments: &self.fragments,
=======
#[serde(deny_unknown_fields)]
>>>>>>>
<<<<<<<
        }
=======
pub struct DetectionResult {
>>>>>>>
<<<<<<<
        .serialize(serializer)
=======
    pub instances: Vec<CloneGroup>,
>>>>>>>
<<<<<<<
    pub stats: DetectionStats,
=======
    }
>>>>>>>
}

<<<<<<<
#[derive(Debug, Clone, Serialize, Deserialize)]
=======
impl<'de> Deserialize<'de> for CloneGroup {
>>>>>>>
<<<<<<<
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
=======
#[serde(deny_unknown_fields)]
>>>>>>>
<<<<<<<
    where
=======
pub struct DetectionStats {
>>>>>>>
<<<<<<<
        D: Deserializer<'de>,
=======
    pub files_scanned: usize,
>>>>>>>
<<<<<<<
    pub nodes_analyzed: usize,
=======
    {
>>>>>>>
<<<<<<<
        #[derive(Deserialize)]
=======
    pub total_lines: usize,
>>>>>>>
<<<<<<<
        #[serde(deny_unknown_fields)]
=======
    pub duplicated_lines: usize,
>>>>>>>
<<<<<<<
        struct Input {
=======
    pub duplication_pct: f64,
>>>>>>>
<<<<<<<
            clone_type: Kind,
=======
    pub type1_groups: usize,
>>>>>>>
<<<<<<<
            #[serde(default)]
=======
    pub type2_groups: usize,
>>>>>>>
<<<<<<<
            impact_lines: Option<usize>,
=======
    pub type3_groups: usize,
>>>>>>>
<<<<<<<
            #[serde(default)]
=======
    pub sequence_groups: usize,
>>>>>>>
<<<<<<<
            generated: Option<bool>,
=======
}
>>>>>>>
<<<<<<<

=======
            fragments: Vec<Fragment>,
>>>>>>>
<<<<<<<
        }
=======
#[derive(Debug, Clone)]
>>>>>>>
<<<<<<<

=======
pub struct DetectConfig {
>>>>>>>
<<<<<<<
        let input = Input::deserialize(deserializer)?;
=======
    pub enable_type3: bool,
>>>>>>>
<<<<<<<
        let group = Self {
=======
    pub type3_threshold: f64,
>>>>>>>
<<<<<<<
            clone_type: input.clone_type,
=======
    pub type3_metric: Type3Metric,
>>>>>>>
<<<<<<<
            fragments: input.fragments,
=======
    pub enable_sequences: bool,
>>>>>>>
<<<<<<<
        };
=======
    pub sequence_window_size: usize,
>>>>>>>
<<<<<<<
        if input.impact_lines.is_some_and(|impact| impact != group.line_impact()) {
=======
}
>>>>>>>
<<<<<<<

=======
            return Err(serde::de::Error::custom("impact_lines does not match fragments"));
>>>>>>>
<<<<<<<
        }
=======
/// Default similarity threshold for Type-3 clone detection.
>>>>>>>
<<<<<<<
        if input.generated.is_some_and(|generated| generated != group.is_generated()) {
=======
const DEFAULT_TYPE3_THRESHOLD: f64 = 0.7;
>>>>>>>
<<<<<<<

=======
            return Err(serde::de::Error::custom("generated does not match fragments"));
>>>>>>>
<<<<<<<
        }
=======
impl Default for DetectConfig {
>>>>>>>
<<<<<<<
        Ok(group)
=======
    fn default() -> Self {
>>>>>>>
<<<<<<<
        Self {
=======
    }
>>>>>>>
<<<<<<<
            enable_type3: false,
=======
}
>>>>>>>
<<<<<<<

=======
            type3_threshold: DEFAULT_TYPE3_THRESHOLD,
>>>>>>>
<<<<<<<
            type3_metric: Type3Metric::default(),
=======
impl CloneGroup {
>>>>>>>
<<<<<<<
            enable_sequences: false,
=======
    /// Estimated lines removable by consolidating this group into its largest
>>>>>>>
<<<<<<<
            sequence_window_size: crate::sequences::DEFAULT_WINDOW_SIZE,
=======
    /// fragment. This is a ranking signal, while the global statistic still
>>>>>>>
<<<<<<<
        }
=======
    /// deduplicates overlapping line ranges across every group.
>>>>>>>
<<<<<<<
    #[must_use]
=======
    }
>>>>>>>
<<<<<<<
    pub fn line_impact(&self) -> usize {
=======
}
>>>>>>>
        let total: usize = self.fragments.iter().map(fragment_line_count).sum();
        let original = self
            .fragments
            .iter()
            .map(fragment_line_count)
            .max()
            .unwrap_or_default();
        total.saturating_sub(original)
    }

    /// True when every fragment is generated output. Mixed authored/generated
    /// groups stay actionable and therefore rank with authored code.
    #[must_use]
    pub fn is_generated(&self) -> bool {
        !self.fragments.is_empty() && self.fragments.iter().all(|fragment| fragment.generated)
    }
}

pub(crate) fn file_is_generated(file: &clone_scanner::File) -> bool {
    let snapshot_path = file
        .path
        .components()
        .any(|component| component.as_os_str() == "snapshots");
    snapshot_path || generated_header(&file.source)
}

pub(crate) fn generated_header(source: &str) -> bool {
    const MAX_HEADER_BYTES: usize = 4096;
    const MARKERS: [&[u8]; 4] = [
        b"automatically generated",
        b"generated by",
        b"do not edit",
        b"@generated",
    ];

    let prefix = &source.as_bytes()[..source.len().min(MAX_HEADER_BYTES)];
    let end = prefix
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(7)
        .map_or(prefix.len(), |(index, _)| index);
    let header = &prefix[..end];

    MARKERS.iter().any(|marker| {
        header
            .windows(marker.len())
            .any(|window| window.eq_ignore_ascii_case(marker))
    })
}

fn fragment_line_count(fragment: &Fragment) -> usize {
    fragment
        .lines
        .end
        .saturating_sub(fragment.lines.start)
        .saturating_add(1)
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
