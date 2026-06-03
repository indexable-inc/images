//! A "story": who you are and what you're currently working on, plus the local
//! state file the `publish`/`serve` commands share.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use color_eyre::Result;
use color_eyre::eyre::{Context, eyre};
use serde::{Deserialize, Serialize};

/// How long a story stays visible, mirroring Instagram's 24h window. A peer
/// whose story is older than this is treated as having no current story.
pub const TTL_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Story {
    /// Display name of the author, e.g. "Andrew Gazelka".
    pub name: String,
    /// Repository the author is working in, e.g. "index".
    pub repo: String,
    /// Current branch.
    pub branch: String,
    /// Subject line of the latest commit: the "what I'm working on" caption.
    pub subject: String,
    /// Unix seconds when the story was published.
    pub ts: i64,
    /// Optional link opened when the avatar is clicked (OSC 8 hyperlink).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl Story {
    /// Whether the story is within the visibility window. `ts` comes from
    /// untrusted peer JSON, so the subtraction is checked (a `ts` of `i64::MIN`
    /// would otherwise overflow) and future-dated stories are rejected.
    #[must_use]
    pub const fn is_fresh(&self, now: i64) -> bool {
        match now.checked_sub(self.ts) {
            Some(age) => age >= 0 && age < TTL_SECS,
            None => false,
        }
    }
}

fn now_secs() -> Result<i64> {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .wrap_err("system clock is before the Unix epoch")?
        .as_secs();
    i64::try_from(secs).wrap_err("timestamp does not fit in i64")
}

/// Derive the current story from a git repository at `path`.
pub fn derive(path: &Path) -> Result<Story> {
    let repo = git2::Repository::discover(path)
        .wrap_err_with(|| format!("no git repository at or above {}", path.display()))?;

    let head = repo.head().wrap_err("repository has no HEAD")?;
    let branch = head.shorthand().unwrap_or("HEAD").to_owned();

    let commit = head
        .peel_to_commit()
        .wrap_err("HEAD does not point at a commit")?;
    let subject = commit.summary().unwrap_or("(no commit message)").to_owned();

    // Prefer the configured user.name; fall back to the latest commit's author.
    let name = repo
        .config()
        .ok()
        .and_then(|c| c.get_string("user.name").ok())
        .or_else(|| commit.author().name().map(str::to_owned))
        .unwrap_or_else(|| "anonymous".to_owned());

    let origin = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().map(str::to_owned));
    let repo_name = origin
        .as_deref()
        .and_then(repo_basename)
        .unwrap_or_else(|| dir_name(&repo));
    let url = origin.as_deref().and_then(github_https);

    Ok(Story {
        name,
        repo: repo_name,
        branch,
        subject,
        ts: now_secs()?,
        url,
    })
}

/// `git@github.com:owner/name.git` or `https://github.com/owner/name` -> "name".
fn repo_basename(remote_url: &str) -> Option<String> {
    let tail = remote_url.rsplit(['/', ':']).next()?;
    Some(tail.strip_suffix(".git").unwrap_or(tail).to_owned())
}

/// Map a GitHub remote URL (ssh or https) to its https web URL, for the OSC 8
/// link target. Returns `None` for non-GitHub or unparseable remotes rather
/// than guessing a URL that might 404.
fn github_https(remote_url: &str) -> Option<String> {
    let rest = remote_url
        .strip_prefix("git@github.com:")
        .or_else(|| remote_url.strip_prefix("https://github.com/"))
        .or_else(|| remote_url.strip_prefix("ssh://git@github.com/"))?;
    let slug = rest.strip_suffix(".git").unwrap_or(rest);
    Some(format!("https://github.com/{slug}"))
}

fn dir_name(repo: &git2::Repository) -> String {
    repo.workdir()
        .and_then(Path::file_name)
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_owned()
}

/// Path to the shared state file. `publish` writes it; `serve` reads it.
pub fn state_path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .ok_or_else(|| eyre!("neither XDG_STATE_HOME nor HOME is set"))?;
    Ok(base.join("claude-stories").join("story.json"))
}

pub fn write_state(story: &Story) -> Result<()> {
    let path = state_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .wrap_err_with(|| format!("creating state dir {}", dir.display()))?;
    }
    let json = serde_json::to_vec_pretty(story)?;
    std::fs::write(&path, json).wrap_err_with(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn read_state() -> Result<Option<Story>> {
    let path = state_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).wrap_err_with(|| format!("reading {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ts: i64) -> Story {
        Story {
            name: "x".into(),
            repo: "r".into(),
            branch: "b".into(),
            subject: "s".into(),
            ts,
            url: None,
        }
    }

    #[test]
    fn freshness_bounds() {
        let now = 1_000_000_i64;
        assert!(at(now).is_fresh(now));
        assert!(at(now - TTL_SECS + 1).is_fresh(now));
        assert!(!at(now - TTL_SECS).is_fresh(now)); // exactly at TTL: expired
        assert!(!at(now + 100).is_fresh(now)); // future-dated peer rejected
        assert!(!at(i64::MIN).is_fresh(now)); // must not overflow/panic
    }
}
