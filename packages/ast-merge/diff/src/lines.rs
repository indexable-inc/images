use rustc_hash::{FxHashMap, FxHashSet};

use crate::conflict::{Conflict, Region, Result};

pub struct Outcome {
    pub content: String,
    pub has_conflict: bool,
}

struct ChangeMaps<'a> {
    deletions: FxHashSet<usize>,
    insertions: FxHashMap<usize, Vec<&'a str>>,
}

fn build_change_maps<'a>(
    changes: impl Iterator<Item = similar::Change<&'a str>>,
) -> ChangeMaps<'a> {
    use similar::ChangeTag;

    let mut deletions: FxHashSet<usize> = FxHashSet::default();
    let mut insertions: FxHashMap<usize, Vec<&'a str>> = FxHashMap::default();
    let mut base_idx = 0;

    for change in changes {
        match change.tag() {
            ChangeTag::Equal => {
                base_idx += 1;
            }
            ChangeTag::Delete => {
                deletions.insert(base_idx);
                base_idx += 1;
            }
            ChangeTag::Insert => {
                insertions.entry(base_idx).or_default().push(change.value());
            }
        }
    }

    ChangeMaps {
        deletions,
        insertions,
    }
}

pub fn inner(base: &str, left: &str, right: &str) -> Outcome {
    use similar::{Algorithm, TextDiff};

    let base_lines: Vec<&str> = base.lines().collect();
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();

    if left_lines == right_lines {
        return Outcome {
            content: left.to_owned(),
            has_conflict: false,
        };
    }

    let diff_base_left = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .diff_slices(&base_lines, &left_lines);
    let diff_base_right = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .diff_slices(&base_lines, &right_lines);

    let ChangeMaps {
        deletions: left_changes,
        insertions: left_insertions,
    } = build_change_maps(diff_base_left.iter_all_changes());
    let ChangeMaps {
        deletions: right_changes,
        insertions: right_insertions,
    } = build_change_maps(diff_base_right.iter_all_changes());

    let mut result = Vec::new();
    let mut has_conflict = false;

    for (i, base_line) in base_lines.iter().enumerate() {
        let left_deleted = left_changes.contains(&i);
        let right_deleted = right_changes.contains(&i);
        let left_insert = left_insertions.get(&i);
        let right_insert = right_insertions.get(&i);

        // Conflict: both sides insert different content at the same position.
        if let (Some(li), Some(ri)) = (left_insert, right_insert)
            && li != ri
        {
            has_conflict = true;
        }

        if let Some(lines) = left_insert {
            for line in lines {
                result.push((*line).to_owned());
            }
        }
        if let Some(lines) = right_insert {
            for line in lines {
                if left_insert.is_none_or(|li| !li.contains(line)) {
                    result.push((*line).to_owned());
                }
            }
        }

        if !left_deleted && !right_deleted {
            result.push((*base_line).to_owned());
        }
    }

    // Check for conflicts in trailing insertions (after last base line).
    let left_trailing = left_insertions.get(&base_lines.len());
    let right_trailing = right_insertions.get(&base_lines.len());
    if let (Some(lt), Some(rt)) = (left_trailing, right_trailing)
        && lt != rt
    {
        has_conflict = true;
    }

    if let Some(lines) = left_trailing {
        for line in lines {
            result.push((*line).to_owned());
        }
    }
    if let Some(lines) = right_trailing {
        for line in lines {
            if left_trailing.is_none_or(|lt| !lt.contains(line)) {
                result.push((*line).to_owned());
            }
        }
    }

    Outcome {
        content: result.join("\n"),
        has_conflict,
    }
}

#[must_use]
pub fn based(base: &str, left: &str, right: &str) -> Result {
    let base_lines: Vec<_> = base.lines().collect();
    let left_lines: Vec<_> = left.lines().collect();
    let right_lines: Vec<_> = right.lines().collect();

    let mut output = String::new();
    let mut conflicts = Vec::new();
    let mut i = 0;

    while i < base_lines.len() || i < left_lines.len() || i < right_lines.len() {
        let base_line = base_lines.get(i).copied();
        let left_line = left_lines.get(i).copied();
        let right_line = right_lines.get(i).copied();

        match (base_line, left_line, right_line) {
            (Some(_), Some(l), Some(r)) if l == r => {
                output.push_str(l);
                output.push('\n');
            }
            (Some(b), Some(l), Some(r)) if l == b => {
                output.push_str(r);
                output.push('\n');
            }
            (Some(b), Some(l), Some(r)) if r == b => {
                output.push_str(l);
                output.push('\n');
            }
            (Some(b), Some(l), Some(r)) => {
                output.push_str("<<<<<<< LEFT\n");
                output.push_str(l);
                output.push('\n');
                output.push_str("||||||| BASE\n");
                output.push_str(b);
                output.push('\n');
                output.push_str("=======\n");
                output.push_str(r);
                output.push('\n');
                output.push_str(">>>>>>> RIGHT\n");
                conflicts.push(Conflict {
                    base: Some(Region::new(0, b.len(), b.to_owned())),
                    left: Region::new(0, l.len(), l.to_owned()),
                    right: Region::new(0, r.len(), r.to_owned()),
                });
            }
            (None, Some(l), Some(r)) if l == r => {
                output.push_str(l);
                output.push('\n');
            }
            (None, Some(l), Some(r)) => {
                output.push_str("<<<<<<< LEFT\n");
                output.push_str(l);
                output.push('\n');
                output.push_str("=======\n");
                output.push_str(r);
                output.push('\n');
                output.push_str(">>>>>>> RIGHT\n");
                conflicts.push(Conflict {
                    base: None,
                    left: Region::new(0, l.len(), l.to_owned()),
                    right: Region::new(0, r.len(), r.to_owned()),
                });
            }
            (Some(_) | None, None, Some(r)) => {
                output.push_str(r);
                output.push('\n');
            }
            (Some(_) | None, Some(l), None) => {
                output.push_str(l);
                output.push('\n');
            }
            _ => {}
        }
        i += 1;
    }

    if conflicts.is_empty() {
        Result::success(output)
    } else {
        Result::with_conflicts(output, conflicts)
    }
}
