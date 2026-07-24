//! Which VCS owns the prompt directory.

use std::path::{Path, PathBuf};

/// The version control system the prompt directory belongs to, and the
/// workspace root that answers queries about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Workspace {
    Jj(PathBuf),
    Git(PathBuf),
}

/// Walk up from `start` looking for a workspace marker, nearest first.
///
/// `.jj` is checked before `.git` *within the same directory* so a colocated
/// repo (every fork this repo maintains) resolves to jj, while a plain git
/// checkout nested inside a jj workspace still resolves to git: the nearer
/// marker wins because the walk stops at the first directory that has either.
pub fn discover(start: &Path) -> Option<Workspace> {
    start.ancestors().find_map(|dir| {
        if dir.join(".jj").exists() {
            Some(Workspace::Jj(dir.to_path_buf()))
        } else if dir.join(".git").exists() {
            Some(Workspace::Git(dir.to_path_buf()))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{Workspace, discover};

    #[test]
    fn colocated_repo_resolves_to_jj() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir(root.path().join(".jj")).expect("create .jj");
        fs::create_dir(root.path().join(".git")).expect("create .git");
        let nested = root.path().join("src/deep");
        fs::create_dir_all(&nested).expect("create nested dirs");

        assert_eq!(
            discover(&nested),
            Some(Workspace::Jj(root.path().to_path_buf()))
        );
    }

    #[test]
    fn nested_git_checkout_wins_over_an_outer_jj_workspace() {
        let root = tempfile::tempdir().expect("tempdir");
        fs::create_dir(root.path().join(".jj")).expect("create .jj");
        let inner = root.path().join("vendor/upstream");
        fs::create_dir_all(inner.join(".git")).expect("create inner .git");

        assert_eq!(discover(&inner), Some(Workspace::Git(inner)));
    }

    #[test]
    fn no_marker_anywhere_is_not_a_workspace() {
        let root = tempfile::tempdir().expect("tempdir");
        assert_eq!(discover(root.path()), None);
    }
}
