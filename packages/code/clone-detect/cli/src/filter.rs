use clone_detect::{
    DetectionResult, DetectionStats, Kind, duplicated_lines, duplication_percentage as ratio_pct,
    rank_by_impact,
};
use clone_scanner::Output;

const MIN_FRAGMENTS: usize = 2;

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

    let mut filtered = DetectionResult {
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
    };
    rank_by_impact(&mut filtered.instances);
    Ok(filtered)
}

#[cfg(test)]
mod tests;
