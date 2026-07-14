//! Repository file discovery with `fd` parity.
//!
//! Gitignore-aware (also without a `.git` directory, which is how the CI
//! lint derivation's source tree arrives), hidden files skipped unless a
//! stage opts in, extension matching case-insensitive and dotfile-friendly
//! (`.editorconfig` matches the `editorconfig` extension). Results are
//! relative to the walk root and sorted for determinism.

use anyhow::{Context, Result};
use ignore::WalkBuilder;

#[derive(Clone, Copy)]
pub enum Hidden {
    Skip,
    Include,
}

#[derive(Clone, Copy)]
enum Kind {
    File,
    Directory,
}

/// Files under the current directory selected by a stage.
pub struct FileQuery<'query> {
    /// Extensions to match, lowercase, without the dot.
    pub extensions: &'query [&'query str],
    pub hidden: Hidden,
    /// Relative paths whose subtrees are never entered (fd `--exclude`).
    pub prune: &'query [&'query str],
}

/// # Errors
/// When the walk cannot read a directory entry.
pub fn files(query: &FileQuery) -> Result<Vec<String>> {
    let extensions = query.extensions;
    collect(".", query.hidden, query.prune, Kind::File, |name| {
        matches_extension(name, extensions)
    })
}

/// Every file with the exact name `name`, hidden directories included
/// (fd `--hidden --glob <name>`).
///
/// # Errors
/// When the walk cannot read a directory entry.
pub fn files_named(name: &str) -> Result<Vec<String>> {
    collect(".", Hidden::Include, &[], Kind::File, |file_name| {
        file_name == name
    })
}

/// Every directory under `root` (fd `--type directory . <root>`), excluding
/// `root` itself.
///
/// # Errors
/// When the walk cannot read a directory entry.
pub fn directories_under(root: &str) -> Result<Vec<String>> {
    collect(root, Hidden::Skip, &[], Kind::Directory, |_| true)
}

fn collect(
    root: &str,
    hidden: Hidden,
    prune: &[&str],
    kind: Kind,
    keep_name: impl Fn(&str) -> bool,
) -> Result<Vec<String>> {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(matches!(hidden, Hidden::Skip))
        // fd parity: apply .gitignore even without a .git directory. The CI
        // lint derivation copies a git-less tree, and the walk must still
        // skip target/, node_modules/, and friends there.
        .require_git(false);
    let prune: Vec<String> = prune.iter().map(|p| format!("{root}/{p}")).collect();
    builder.filter_entry(move |entry| {
        let path = entry.path().to_string_lossy();
        !prune.iter().any(|skip| path.as_ref() == skip)
    });

    let mut found = Vec::new();
    for entry in builder.build() {
        let entry = entry.context("walk repository files")?;
        if entry.depth() == 0 {
            continue;
        }
        let is_dir = entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir());
        if matches!(kind, Kind::File) == is_dir {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if !keep_name(&name) {
            continue;
        }
        let path = entry
            .path()
            .to_str()
            .context("repository path is not valid UTF-8")?;
        found.push(path.strip_prefix("./").unwrap_or(path).to_owned());
    }
    found.sort();
    Ok(found)
}

/// fd-style extension matching: case-insensitive, and a leading-dot file like
/// `.editorconfig` matches the `editorconfig` extension.
fn matches_extension(name: &str, extensions: &[&str]) -> bool {
    let lower = name.to_ascii_lowercase();
    extensions.iter().any(|extension| {
        lower
            .strip_suffix(extension)
            .is_some_and(|prefix| prefix.ends_with('.') && !prefix.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::matches_extension;

    #[test]
    fn extension_matching_is_case_insensitive_and_dotfile_friendly() {
        assert!(matches_extension("Cargo.toml", &["toml"]));
        assert!(matches_extension("FLAKE.TOML", &["toml"]));
        assert!(matches_extension(".editorconfig", &["editorconfig"]));
        assert!(!matches_extension("editorconfig", &["editorconfig"]));
        assert!(!matches_extension("sobelow-conf", &["conf"]));
        assert!(matches_extension("x.sobelow-conf", &["sobelow-conf"]));
    }
}
