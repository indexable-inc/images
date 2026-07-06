use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use clone_detect::{DetectionResult, DetectionStats, Kind};
use clone_scanner::Output;

const MIN_FRAGMENTS: usize = 2;
/// Multiplier turning a ratio into a percentage (mirrors `detect`).
const PERCENT: f64 = 100.0;

/// Drop fragments in ignored files, then **recompute** the duplication stats
/// over what survives.
///
/// The detector computes `duplication_pct` over the whole scan; if we only
/// dropped fragments and copied the pre-filter stats, the ignore globs would
/// not move the number the gate keys on (the metric would be identical with and
/// without ignores). So this recomputes:
/// - `duplicated_lines` from the surviving groups only (same per-file,
///   skip-the-original dedup as `detect::compute_duplicated_lines`), and
/// - `total_lines` over the **gated** files only — files matching an ignore
///   glob are removed from the denominator too, so the metric reads as
///   "duplication among the code the gate actually covers", not diluted by
///   vendored/generated lines that can never contribute clones.
///
/// `files_scanned` and `nodes_analyzed` still describe the raw scan (they are
/// not gate inputs), so they are passed through unchanged.
pub fn by_patterns(
    result: DetectionResult,
    scan: &Output,
    patterns: &[glob::Pattern],
) -> Result<DetectionResult, crate::RunError> {
    if patterns.is_empty() {
        return Ok(result);
    }

    let matches_ignore = |path: &std::path::Path| -> Result<bool, crate::RunError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| crate::RunError::NonUtf8Path {
                path: path.to_path_buf(),
            })?;
        Ok(patterns.iter().any(|pattern| pattern.matches(path_str)))
    };

    let mut filtered_clones = Vec::new();
    for mut clone in result.instances {
        let mut fragments = Vec::with_capacity(clone.fragments.len());
        for fragment in clone.fragments {
            if !matches_ignore(&fragment.file)? {
                fragments.push(fragment);
            }
        }
        clone.fragments = fragments;
        if clone.fragments.len() >= MIN_FRAGMENTS {
            filtered_clones.push(clone);
        }
    }

    let mut type1_groups = 0;
    let mut type2_groups = 0;
    let mut type3_groups = 0;
    let mut sequence_groups = 0;
    for clone in &filtered_clones {
        match clone.clone_type {
            Kind::Type1 => type1_groups += 1,
            Kind::Type2 => type2_groups += 1,
            Kind::Type3 { .. } => type3_groups += 1,
            Kind::Sequence { .. } => sequence_groups += 1,
        }
    }

    // Denominator: lines of gated files only.
    let mut total_lines = 0_usize;
    for file in &scan.files {
        if !matches_ignore(&file.path)? {
            total_lines += file.source.lines().count();
        }
    }

    let duplicated_lines = duplicated_lines(&filtered_clones);
    let duplication_pct = ratio_pct(duplicated_lines, total_lines);

    Ok(DetectionResult {
        instances: filtered_clones,
        stats: DetectionStats {
            files_scanned: result.stats.files_scanned,
            nodes_analyzed: result.stats.nodes_analyzed,
            total_lines,
            duplicated_lines,
            duplication_pct,
            type1_groups,
            type2_groups,
            type3_groups,
            sequence_groups,
        },
    })
}

/// Deduplicated duplicated-line count over the surviving groups. Mirrors
/// `detect::compute_duplicated_lines`: per group skip the first fragment (the
/// "original"), and dedup line numbers per file across all groups.
fn duplicated_lines(instances: &[clone_detect::CloneGroup]) -> usize {
    let mut per_file: BTreeMap<&PathBuf, BTreeSet<usize>> = BTreeMap::new();
    for group in instances {
        for frag in group.fragments.iter().skip(1) {
            let lines = per_file.entry(&frag.file).or_default();
            for line in frag.lines.start..=frag.lines.end {
                lines.insert(line);
            }
        }
    }
    per_file.values().map(BTreeSet::len).sum()
}

/// `100 * numerator / denominator`, `0.0` when the denominator is zero.
#[expect(
    clippy::cast_precision_loss,
    reason = "line counts stay far below f64 mantissa precision"
)]
fn ratio_pct(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64 / denominator as f64) * PERCENT
    }
}

#[cfg(test)]
mod tests;
