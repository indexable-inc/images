//! Render a set of changed file paths as a colored, icon-annotated tree.
//!
//! Paths are folded into a directory trie, single-child directory chains are
//! collapsed (so `a/b/c.rs` shows as one `a/b` node), and each file gets its
//! [`devicons`] glyph in the icon's own color. Directory segments render gray so
//! the filename stays the focus.

use std::collections::BTreeMap;

use anstyle::Color;
use devicons::icon_for_file;

use crate::palette::{self, GRAY, Theme};

/// Closed-folder glyph (Nerd Font `nf-md-folder`), shown for directory nodes.
const FOLDER_GLYPH: &str = "\u{e5ff}";

/// A node in the path trie. Leaves are files; interior nodes are directories.
#[derive(Default)]
struct Node {
    is_file: bool,
    children: BTreeMap<String, Self>,
}

/// Render `files` as a tree, one line per node, joined by newlines. Returns an
/// empty string when there are no files so callers can skip printing.
pub fn render(files: &[String], theme: Theme) -> String {
    if files.is_empty() {
        return String::new();
    }

    let mut root = Node::default();
    for path in files {
        insert(&mut root, path);
    }
    collapse(&mut root);

    let mut lines = Vec::new();
    render_children(&root, theme, "    ", &mut lines);
    lines.join("\n")
}

/// Insert one slash-separated path into the trie, marking the final segment as a
/// file.
fn insert(root: &mut Node, path: &str) {
    let parts: Vec<&str> = path.split('/').collect();
    let mut node = root;

    for (index, part) in parts.iter().enumerate() {
        let is_last = index == parts.len() - 1;
        node = node.children.entry((*part).to_string()).or_default();
        node.is_file = node.is_file || is_last;
    }
}

/// Collapse single-child directory chains so a deep unique path renders as one
/// `dir/dir/leaf` node instead of nested connectors.
fn collapse(node: &mut Node) {
    let merges: Vec<(String, String)> = node
        .children
        .iter_mut()
        .filter_map(|(name, child)| {
            collapse(child);
            let only_child = (!child.is_file && child.children.len() == 1)
                .then(|| child.children.keys().next().cloned())
                .flatten();
            only_child.map(|child_name| (name.clone(), child_name))
        })
        .collect();

    for (dir_name, child_name) in merges {
        let Some(dir_node) = node.children.remove(&dir_name) else {
            continue;
        };
        let Some(grandchild) = dir_node.children.into_values().next() else {
            continue;
        };
        node.children.insert(format!("{dir_name}/{child_name}"), grandchild);
    }
}

/// Render each child of `node`, drawing the `├──`/`└──` connector and recursing
/// with an extended prefix for grandchildren.
fn render_children(node: &Node, theme: Theme, prefix: &str, lines: &mut Vec<String>) {
    let count = node.children.len();

    for (index, (name, child)) in node.children.iter().enumerate() {
        let is_last = index == count - 1;
        let connector = if is_last { "└── " } else { "├── " };

        let label = node_label(name, child, theme);
        lines.push(format!(
            "{prefix}{connector}{label}",
            connector = palette::paint(palette::fg(Color::Rgb(GRAY)), connector),
        ));

        if !child.children.is_empty() {
            let continuation = if is_last {
                " ".to_string()
            } else {
                palette::paint(palette::fg(Color::Rgb(GRAY)), "│")
            };
            let next_prefix = format!("{prefix}{continuation}    ");
            render_children(child, theme, &next_prefix, lines);
        }
    }
}

/// Build the styled label for one node: a gray directory name, or a file name
/// (gray directory segments, high-contrast basename) followed by its colored
/// icon. The basename follows the detected theme so it stays readable on light
/// terminals (black) as well as dark ones (white).
fn node_label(name: &str, child: &Node, theme: Theme) -> String {
    let basename_fg = Color::Rgb(palette::chip_foreground(theme));
    let gray = palette::fg(Color::Rgb(GRAY));

    if !child.is_file {
        return format!(
            "{} {}",
            palette::paint(gray, FOLDER_GLYPH),
            palette::paint(gray, name),
        );
    }

    let icon = icon_for_file(name, &Some(palette::devicons(theme)));
    let icon_style = palette::fg(Color::Rgb(palette::parse_hex(icon.color)));

    let name_part = name.rfind('/').map_or_else(
        || palette::paint(palette::fg(basename_fg), name),
        |slash| {
            format!(
                "{}{}",
                palette::paint(gray, &name[..=slash]),
                palette::paint(palette::fg(basename_fg), &name[slash + 1..]),
            )
        },
    );

    format!(
        "{name_part} {icon}",
        icon = palette::paint(icon_style, &icon.icon.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip SGR sequences so structural assertions ignore color.
    fn plain(styled: &str) -> String {
        let bytes = strip_ansi_escapes::strip(styled);
        String::from_utf8(bytes).unwrap()
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert_eq!(render(&[], Theme::Dark), "");
    }

    #[test]
    fn single_chain_collapses_into_one_node() {
        let rendered = plain(&render(&["a/b/c.rs".to_string()], Theme::Dark));
        assert!(rendered.contains("a/b/c.rs"), "got: {rendered}");
        assert_eq!(rendered.lines().count(), 1, "got: {rendered}");
    }

    #[test]
    fn siblings_use_branch_and_last_connectors() {
        let rendered = plain(&render(
            &["src/a.rs".to_string(), "src/b.rs".to_string()],
            Theme::Dark,
        ));
        assert!(rendered.contains("├── "), "got: {rendered}");
        assert!(rendered.contains("└── "), "got: {rendered}");
    }
}
