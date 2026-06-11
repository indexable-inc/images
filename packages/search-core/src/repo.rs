//! Repository identity for code records.
//!
//! Cross-repo vs single-repo scoping needs a stable name per checkout. We derive
//! it from the `origin` remote (so every worktree of one repo shares a slug like
//! `indexable-inc/index`) and fall back to the checkout's directory name when
//! there is no remote. The fallback is observable as [`RepoSlug::Local`], never a
//! silent empty string.

use std::path::Path;

use source_meta::RepoSlug;

/// Resolve the repository slug for the checkout rooted at `root`.
///
/// Uses the `origin` remote URL when present (any worktree resolves to the same
/// slug), otherwise the directory name.
#[must_use]
pub fn repo_slug(root: &Path) -> RepoSlug {
    if let Some(slug) = origin_slug(root) {
        return RepoSlug::Remote(slug);
    }
    let name = root.file_name().map_or_else(
        || root.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    );
    RepoSlug::Local(name)
}

fn origin_slug(root: &Path) -> Option<String> {
    let repo = git2::Repository::discover(root).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    slug_from_url(remote.url().ok()?)
}

/// Extract `owner/repo` from a git remote URL, dropping a trailing `.git`.
///
/// Handles both `https://host/owner/repo(.git)` and `git@host:owner/repo(.git)`.
fn slug_from_url(url: &str) -> Option<String> {
    let url = url.trim();
    let url = url.strip_suffix(".git").unwrap_or(url);
    // Reduce to the path after the host. The URL form `scheme://host/owner/repo`
    // drops the leading host segment; the scp form `user@host:owner/repo` takes
    // the part after the colon.
    let path = match url.split_once("://") {
        Some((_, after)) => after.split_once('/').map(|(_, path)| path)?,
        None => url.split_once(':').map_or(url, |(_, after)| after),
    };
    let mut segments = path.rsplit('/').filter(|segment| !segment.is_empty());
    let repo = segments.next()?;
    let owner = segments.next()?;
    Some(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::slug_from_url;

    #[test]
    fn https_url() {
        assert_eq!(
            slug_from_url("https://github.com/indexable-inc/index.git").as_deref(),
            Some("indexable-inc/index")
        );
        assert_eq!(
            slug_from_url("https://github.com/indexable-inc/index").as_deref(),
            Some("indexable-inc/index")
        );
    }

    #[test]
    fn scp_url() {
        assert_eq!(
            slug_from_url("git@github.com:indexable-inc/index.git").as_deref(),
            Some("indexable-inc/index")
        );
    }

    #[test]
    fn too_short_is_none() {
        assert_eq!(slug_from_url("https://example.com/justone"), None);
    }
}
