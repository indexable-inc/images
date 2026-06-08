//! Render a set of changed file paths as a colored, icon-annotated tree.
//!
//! Paths are folded into a directory trie, single-child directory chains are
//! collapsed (so `a/b/c.rs` shows as one `a/b` node), and each file gets its
//! [`devicons`] glyph in the icon's own color. Directory segments render gray so
//! the filename stays the focus.

use std::collections::BTreeMap;

use anstyle::Color;
use devicons::icon_for_file;

use crate::git::{ChangeKind, ChangedFile};
use crate::palette::{self, GRAY, Theme};

/// Closed-folder glyph (Nerd Font `nf-md-folder`), shown for directory nodes.
const FOLDER_GLYPH: &str = "\u{e5ff}";

/// A node in the path trie. Leaves are files (`change` carries how the file was
/// touched); interior nodes are directories.
#[derive(Default)]
struct Node {
    is_file: bool,
    change: Option<ChangeKind>,
    children: BTreeMap<String, Self>,
}

/// Render `files` as a tree, one line per node, joined by newlines. Returns an
/// empty string when there are no files so callers can skip printing.
pub fn render(files: &[ChangedFile], theme: Theme) -> String {
    if files.is_empty() {
        return String::new();
    }

    let mut root = Node::default();
    for file in files {
        insert(&mut root, file);
    }
    collapse(&mut root);

    let mut lines = Vec::new();
    render_children(&root, theme, "  ", &mut lines);
    lines.join("\n")
}

/// Insert one changed file into the trie, marking the final path segment as a
/// file and recording how it changed so the leaf can be styled.
fn insert(root: &mut Node, file: &ChangedFile) {
    let parts: Vec<&str> = file.path.split('/').collect();
    let mut node = root;

    for (index, part) in parts.iter().enumerate() {
        let is_last = index == parts.len() - 1;
        node = node.children.entry((*part).to_string()).or_default();
        if is_last {
            node.is_file = true;
            node.change = Some(file.kind);
        }
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

/// Render each child of `node`, drawing the `├`/`└` connector and recursing with
/// an extended prefix for grandchildren. Each level adds a 2-column step so deep
/// trees stay compact.
fn render_children(node: &Node, theme: Theme, prefix: &str, lines: &mut Vec<String>) {
    let count = node.children.len();

    for (index, (name, child)) in node.children.iter().enumerate() {
        let is_last = index == count - 1;
        let connector = if is_last { "└ " } else { "├ " };

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
            let next_prefix = format!("{prefix}{continuation} ");
            render_children(child, theme, &next_prefix, lines);
        }
    }
}

/// Build the styled label for one node: a gray directory name, or a file name
/// (gray directory segments, high-contrast basename) followed by its colored
/// icon. The basename follows the detected theme so it stays readable on light
/// terminals (black) as well as dark ones (white).
///
/// A deleted file is grayed out and struck through end to end — directory
/// segments, basename, and icon alike — so a removal reads at a glance and never
/// competes for attention with the files that still exist.
fn node_label(name: &str, child: &Node, theme: Theme) -> String {
    let gray = palette::fg(Color::Rgb(GRAY));

    if !child.is_file {
        return format!(
            "{} {}",
            palette::paint(gray, FOLDER_GLYPH),
            palette::paint(gray, name),
        );
    }

    let deleted = child.change == Some(ChangeKind::Deleted);
    let dir_style = if deleted { gray.strikethrough() } else { gray };
    let basename_style = if deleted {
        gray.strikethrough()
    } else {
        palette::fg(Color::Rgb(palette::chip_foreground(theme)))
    };

    let icon = icon_for_file(name, &Some(palette::devicons(theme)));
    let icon_style = if deleted {
        gray.strikethrough()
    } else {
        palette::fg(Color::Rgb(palette::parse_hex(icon.color)))
    };

    let name_part = name.rfind('/').map_or_else(
        || palette::paint(basename_style, name),
        |slash| {
            format!(
                "{}{}",
                palette::paint(dir_style, &name[..=slash]),
                palette::paint(basename_style, &name[slash + 1..]),
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

    /// A file touched without being removed, the common case.
    fn modified(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.to_string(),
            kind: ChangeKind::Modified,
        }
    }

    /// A removed file, which the renderer grays out and strikes through.
    fn deleted(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.to_string(),
            kind: ChangeKind::Deleted,
        }
    }

    #[test]
    fn empty_input_renders_nothing() {
        assert_eq!(render(&[], Theme::Dark), "");
    }

    #[test]
    fn single_chain_collapses_into_one_node() {
        let rendered = plain(&render(&[modified("a/b/c.rs")], Theme::Dark));
        assert!(rendered.contains("a/b/c.rs"), "got: {rendered}");
        assert_eq!(rendered.lines().count(), 1, "got: {rendered}");
    }

    #[test]
    fn siblings_use_branch_and_last_connectors() {
        let rendered = plain(&render(
            &[modified("src/a.rs"), modified("src/b.rs")],
            Theme::Dark,
        ));
        assert!(rendered.contains("├ "), "got: {rendered}");
        assert!(rendered.contains("└ "), "got: {rendered}");
    }

    /// The strikethrough SGR prefix (`\x1b[9m`) the deleted style emits, used to
    /// assert styling without depending on the surrounding color parameters.
    fn strike_prefix() -> String {
        palette::fg(Color::Rgb(GRAY)).strikethrough().render().to_string()
    }

    #[test]
    fn deleted_file_is_struck_through_but_keeps_its_name() {
        let styled = render(&[deleted("src/gone.rs")], Theme::Dark);
        // The path still reads plainly once SGR is stripped...
        assert!(plain(&styled).contains("gone.rs"), "got: {}", plain(&styled));
        // ...but the styled output carries the strikethrough effect.
        assert!(styled.contains(&strike_prefix()), "deleted file not struck through");
    }

    #[test]
    fn surviving_file_is_not_struck_through() {
        let styled = render(&[modified("src/stays.rs")], Theme::Dark);
        assert!(!styled.contains(&strike_prefix()), "modified file should not be struck through");
    }
}
