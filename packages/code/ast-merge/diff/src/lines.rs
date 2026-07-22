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

/// One side's edit to the base: the base line range it replaces and the
/// replacement line range in that side's file.
struct Hunk {
    base: std::ops::Range<usize>,
    side: std::ops::Range<usize>,
}

fn hunks(base: &[&str], side: &[&str]) -> Vec<Hunk> {
    use similar::{Algorithm, DiffOp, capture_diff_slices};

    capture_diff_slices(Algorithm::Patience, base, side)
        .into_iter()
        .filter(|op| !matches!(op, DiffOp::Equal { .. }))
        .map(|op| Hunk {
            base: op.old_range(),
            side: op.new_range(),
        })
        .collect()
}

fn push_lines(output: &mut String, lines: &[&str]) {
    for line in lines {
        output.push_str(line);
        output.push('\n');
    }
}

/// Walks one side's hunks in lockstep with the shared base cursor. Left and
/// right are structurally symmetric (each just diffs its own lines against
/// the same base), so `based()` drives two of these instead of duplicating
/// the grow-and-measure logic per side.
struct SideCursor<'a> {
    hunks: &'a [Hunk],
    next: usize,
    last: Option<&'a Hunk>,
    pos: usize,
}

impl<'a> SideCursor<'a> {
    const fn new(hunks: &'a [Hunk]) -> Self {
        Self {
            hunks,
            next: 0,
            last: None,
            pos: 0,
        }
    }

    fn peek_start(&self) -> Option<usize> {
        self.hunks.get(self.next).map(|hunk| hunk.base.start)
    }

    /// Consumes every not-yet-seen hunk starting at or before `end`, growing
    /// `end` to cover it. Returns whether it consumed anything, so the
    /// caller can alternate sides until neither grows the region further.
    fn consume(&mut self, end: &mut usize) -> bool {
        let mut grew = false;
        while let Some(hunk) = self.hunks.get(self.next)
            && hunk.base.start <= *end
        {
            *end = (*end).max(hunk.base.end);
            self.last = Some(hunk);
            self.next += 1;
            grew = true;
        }
        grew
    }

    /// This side's text end for the region `[region_start, end)`: the last
    /// consumed hunk's replacement end plus any stable tail up to `end`, or
    /// (no hunk touched this region) `pos` tracking the base 1:1.
    fn region_end(&self, region_start: usize, end: usize) -> usize {
        self.last.map_or(self.pos + (end - region_start), |hunk| {
            hunk.side.end + (end - hunk.base.end)
        })
    }
}

/// Three-way line merge with diff3 alignment.
///
/// Each side is diffed against the base (patience diff), so an insertion
/// early in one side shifts nothing: disjoint edits apply cleanly and only
/// overlapping or touching edits that disagree become conflicts. Touching
/// edit regions coalesce into one conflict, matching GNU diff3 / `git
/// merge-file`, because their relative order is ambiguous (index#3762: the
/// previous positional walk conflicted nearly every line of a file after a
/// single early insertion).
#[must_use]
pub fn based(base: &str, left: &str, right: &str) -> Result {
    if left == right {
        return Result::success(left.to_owned());
    }
    if base == left {
        return Result::success(right.to_owned());
    }
    if base == right {
        return Result::success(left.to_owned());
    }

    let base_lines: Vec<&str> = base.lines().collect();
    let left_lines: Vec<&str> = left.lines().collect();
    let right_lines: Vec<&str> = right.lines().collect();

    let left_hunks = hunks(&base_lines, &left_lines);
    let right_hunks = hunks(&base_lines, &right_lines);

    let mut output = String::new();
    let mut conflicts = Vec::new();

    // `cursor` walks base lines; each side's cursor tracks `pos`, its own
    // line index, staying aligned with `cursor` whenever `cursor` sits
    // outside any hunk.
    let mut cursor = 0;
    let mut left = SideCursor::new(&left_hunks);
    let mut right = SideCursor::new(&right_hunks);

    loop {
        let next = [left.peek_start(), right.peek_start()]
            .into_iter()
            .flatten()
            .min();
        let Some(next) = next else { break };

        push_lines(&mut output, &base_lines[cursor..next]);
        left.pos += next - cursor;
        right.pos += next - cursor;
        let region_start = next;

        // Grow the unstable region to a maximal chunk: consuming a hunk from
        // one side can extend `end` into range of the other side's next hunk,
        // so repeat until neither side adds one.
        let mut end = region_start;
        left.last = None;
        right.last = None;
        while left.consume(&mut end) | right.consume(&mut end) {}

        let left_end = left.region_end(region_start, end);
        let right_end = right.region_end(region_start, end);

        let base_seg = &base_lines[region_start..end];
        let left_seg = &left_lines[left.pos..left_end];
        let right_seg = &right_lines[right.pos..right_end];

        if left_seg == right_seg {
            push_lines(&mut output, left_seg);
        } else if left_seg == base_seg {
            push_lines(&mut output, right_seg);
        } else if right_seg == base_seg {
            push_lines(&mut output, left_seg);
        } else {
            output.push_str("<<<<<<< LEFT\n");
            push_lines(&mut output, left_seg);
            output.push_str("||||||| BASE\n");
            push_lines(&mut output, base_seg);
            output.push_str("=======\n");
            push_lines(&mut output, right_seg);
            output.push_str(">>>>>>> RIGHT\n");
            conflicts.push(Conflict {
                base: (!base_seg.is_empty())
                    .then(|| Region::new(region_start, end, base_seg.join("\n"))),
                left: Region::new(left.pos, left_end, left_seg.join("\n")),
                right: Region::new(right.pos, right_end, right_seg.join("\n")),
            });
        }

        cursor = end;
        left.pos = left_end;
        right.pos = right_end;
    }

    push_lines(&mut output, &base_lines[cursor..]);

    if conflicts.is_empty() {
        Result::success(output)
    } else {
        Result::with_conflicts(output, conflicts)
    }
}
