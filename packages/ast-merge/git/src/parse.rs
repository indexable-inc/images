use lazy_regex::regex;

use crate::types::{ParsedConflict, ParsedFile};

fn left_marker() -> &'static lazy_regex::Regex {
    regex!(r"^<{7,} (.*)$")
}
fn base_marker() -> &'static lazy_regex::Regex {
    regex!(r"^\|{7,} (.*)$")
}
fn separator() -> &'static lazy_regex::Regex {
    regex!(r"^={7,}$")
}
fn right_marker() -> &'static lazy_regex::Regex {
    regex!(r"^>{7,} (.*)$")
}

#[must_use]
pub fn conflicts(content: &str) -> ParsedFile {
    let mut conflicts = Vec::new();
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let Some(captures) = left_marker().captures(lines.get(i).copied().unwrap_or_default())
        else {
            i += 1;
            continue;
        };

        let left_name = captures.get(1).map_or("LEFT", |m| m.as_str()).to_owned();
        let conflict_start = i;
        i += 1;

        let left_result = read_left_section(&lines, i);
        i = left_result.next_index;

        let base_result = read_base_section(&lines, i, left_result.base_lines);
        i = base_result.next_index;
        let base_lines = base_result.base_lines;
        let base_name = left_result.base_name;
        let left_lines = left_result.left_lines;

        let right_result = read_right_section(&lines, i);
        i = right_result.next_index;

        let before = lines
            .get(..conflict_start)
            .map_or(String::new(), |l| l.join("\n"));
        let after = lines.get(i..).map_or(String::new(), |l| l.join("\n"));

        conflicts.push(ParsedConflict {
            before,
            left: left_lines.join("\n"),
            base: base_lines.map(|l| l.join("\n")),
            right: right_result.right_lines.join("\n"),
            after,
            left_name,
            right_name: right_result.right_name,
            base_name,
        });
    }

    ParsedFile {
        has_conflicts: !conflicts.is_empty(),
        conflicts,
        content: content.to_owned(),
    }
}

struct LeftSectionResult<'a> {
    left_lines: Vec<&'a str>,
    base_lines: Option<Vec<&'a str>>,
    base_name: Option<String>,
    next_index: usize,
}

fn read_left_section<'a>(lines: &[&'a str], mut i: usize) -> LeftSectionResult<'a> {
    let mut left_lines = Vec::new();

    while i < lines.len() {
        let line = lines.get(i).copied().unwrap_or_default();

        if let Some(captures) = base_marker().captures(line) {
            let base_name = captures.get(1).map(|m| m.as_str().to_owned());
            return LeftSectionResult {
                left_lines,
                base_lines: Some(Vec::new()),
                base_name,
                next_index: i + 1,
            };
        }

        if separator().is_match(line) {
            return LeftSectionResult {
                left_lines,
                base_lines: None,
                base_name: None,
                next_index: i + 1,
            };
        }

        left_lines.push(line);
        i += 1;
    }

    LeftSectionResult {
        left_lines,
        base_lines: None,
        base_name: None,
        next_index: i,
    }
}

struct BaseSectionResult<'a> {
    base_lines: Option<Vec<&'a str>>,
    next_index: usize,
}

fn read_base_section<'a>(
    lines: &[&'a str],
    mut i: usize,
    base_lines: Option<Vec<&'a str>>,
) -> BaseSectionResult<'a> {
    let Some(mut base) = base_lines else {
        return BaseSectionResult {
            base_lines: None,
            next_index: i,
        };
    };

    while i < lines.len() {
        let line = lines.get(i).copied().unwrap_or_default();
        if separator().is_match(line) {
            return BaseSectionResult {
                base_lines: Some(base),
                next_index: i + 1,
            };
        }
        base.push(line);
        i += 1;
    }

    BaseSectionResult {
        base_lines: Some(base),
        next_index: i,
    }
}

struct RightSectionResult<'a> {
    right_lines: Vec<&'a str>,
    right_name: String,
    next_index: usize,
}

fn read_right_section<'a>(lines: &[&'a str], mut i: usize) -> RightSectionResult<'a> {
    let mut right_lines = Vec::new();

    while i < lines.len() {
        let line = lines.get(i).copied().unwrap_or_default();
        if let Some(captures) = right_marker().captures(line) {
            let right_name = captures.get(1).map_or("RIGHT", |m| m.as_str()).to_owned();
            return RightSectionResult {
                right_lines,
                right_name,
                next_index: i + 1,
            };
        }
        right_lines.push(line);
        i += 1;
    }

    RightSectionResult {
        right_lines,
        right_name: String::from("RIGHT"),
        next_index: i,
    }
}
