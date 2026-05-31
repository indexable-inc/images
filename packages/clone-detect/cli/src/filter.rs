use clone_detect::{DetectionResult, DetectionStats, Kind};

const MIN_FRAGMENTS: usize = 2;

pub fn by_patterns(
    result: DetectionResult,
    patterns: &[glob::Pattern],
) -> Result<DetectionResult, crate::RunError> {
    if patterns.is_empty() {
        return Ok(result);
    }

    let mut filtered_clones = Vec::new();

    for mut clone in result.instances {
        let mut fragments = Vec::with_capacity(clone.fragments.len());

        for fragment in clone.fragments {
            let Some(path_str) = fragment.file.to_str() else {
                return Err(crate::RunError::NonUtf8Path {
                    path: fragment.file,
                });
            };

            if !patterns.iter().any(|pattern| pattern.matches(path_str)) {
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

    Ok(DetectionResult {
        instances: filtered_clones,
        stats: DetectionStats {
            files_scanned: result.stats.files_scanned,
            nodes_analyzed: result.stats.nodes_analyzed,
            total_lines: result.stats.total_lines,
            duplicated_lines: result.stats.duplicated_lines,
            duplication_pct: result.stats.duplication_pct,
            type1_groups,
            type2_groups,
            type3_groups,
            sequence_groups,
        },
    })
}
