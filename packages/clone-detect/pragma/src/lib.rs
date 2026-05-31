use std::ops::Range;

use ast_merge_ast::Tree;

const COMMENT_KINDS: &[&str] = &[
    "comment",
    "line_comment",
    "block_comment",
    "comment",
    "comment",
];

fn is_comment(kind: &str) -> bool {
    COMMENT_KINDS.contains(&kind)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pragma {
    Ignore,
    IgnoreStart,
    IgnoreEnd,
    IgnoreFile,
}

pub(crate) fn parse_text(text: &str) -> Option<Pragma> {
    let text = text
        .trim()
        .trim_start_matches("//")
        .trim_start_matches('#')
        .trim_start_matches("/*")
        .trim_end_matches("*/")
        .trim_start_matches("--")
        .trim();

    let pragma = text.strip_prefix("clone:")?;
    // Reject "clone: ignore" (space after colon) — pragmas must be tight like "clone:ignore".
    match pragma
        .split_once(char::is_whitespace)
        .map_or(pragma, |(word, _)| word)
    {
        "ignore-file" => Some(Pragma::IgnoreFile),
        "ignore-start" => Some(Pragma::IgnoreStart),
        "ignore-end" => Some(Pragma::IgnoreEnd),
        "ignore" => Some(Pragma::Ignore),
        _ => None,
    }
}

#[derive(Debug, Default)]
pub struct Info {
    pub ignore_file: bool,
    pub ignored_ranges: Vec<Range<usize>>,
}

impl Info {
    #[must_use]
    pub fn is_ignored(&self, range: &Range<usize>) -> bool {
        if self.ignore_file {
            return true;
        }
        self.ignored_ranges
            .iter()
            .any(|ignored| ranges_overlap(ignored, range))
    }
}

const fn ranges_overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

struct ScanState {
    in_ignore_region: bool,
    ignore_region_start: Option<usize>,
    ignore_next: Option<usize>,
}

struct ScanContext<'a> {
    tree: &'a Tree,
    info: &'a mut Info,
    state: &'a mut ScanState,
}

#[must_use]
pub fn scan(tree: &Tree) -> Info {
    let mut info = Info::default();
    let mut state = ScanState {
        in_ignore_region: false,
        ignore_region_start: None,
        ignore_next: None,
    };

    let root = tree.root_node();
    scan_recursive(
        &mut ScanContext {
            tree,
            info: &mut info,
            state: &mut state,
        },
        root,
    );

    if let Some(start) = state.ignore_region_start {
        info.ignored_ranges.push(start..tree.source().len());
    }

    info
}

fn scan_recursive(ctx: &mut ScanContext<'_>, node: tree_sitter::Node<'_>) {
    let kind = node.kind();

    if let Some(comment_end) = ctx.state.ignore_next
        && node.start_byte() >= comment_end
        && !is_comment(kind)
    {
        ctx.info.ignored_ranges.push(node.byte_range());
        ctx.state.ignore_next = None;
    }

    if is_comment(kind) {
        let text = ctx.tree.node_text(node);
        if let Some(pragma) = parse_text(text) {
            match pragma {
                Pragma::IgnoreFile => {
                    ctx.info.ignore_file = true;
                }
                Pragma::Ignore => {
                    ctx.state.ignore_next = Some(node.end_byte());
                }
                Pragma::IgnoreStart => {
                    ctx.state.in_ignore_region = true;
                    ctx.state.ignore_region_start = Some(node.end_byte());
                }
                Pragma::IgnoreEnd => {
                    if let Some(start) = ctx.state.ignore_region_start.take() {
                        ctx.info.ignored_ranges.push(start..node.start_byte());
                    }
                    ctx.state.in_ignore_region = false;
                }
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        scan_recursive(ctx, child);
    }
}

#[cfg(test)]
#[expect(
    clippy::string_slice,
    clippy::single_range_in_vec_init,
    reason = "Test code with controlled inputs"
)]
mod tests;
