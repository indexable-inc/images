use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use tokio::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSnapshot {
    pub cwd: String,
    pub root: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub base_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangedFile {
    pub path: String,
    pub status: String,
    pub additions: i64,
    pub deletions: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileContents {
    pub contents: String,
    pub truncated: bool,
}

async fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

async fn git_optional(cwd: &Path, args: &[&str]) -> Option<String> {
    git(cwd, args).await.ok().filter(|s| !s.is_empty())
}

async fn git_diff(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .with_context(|| format!("run git {}", args.join(" ")))?;
    if !out.status.success() && out.status.code() != Some(1) {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

pub async fn snapshot(cwd: Option<&str>) -> Result<WorkspaceSnapshot> {
    let cwd = cwd
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let root = git(&cwd, &["rev-parse", "--show-toplevel"]).await?;
    let root_path = PathBuf::from(&root);
    let repo = root_path
        .file_name()
        .map(|s| s.to_string_lossy().to_string());
    let branch = git_optional(&root_path, &["branch", "--show-current"]).await;
    let base_sha = git_optional(&root_path, &["merge-base", "HEAD", "origin/main"])
        .await
        .or_else(|| None);
    Ok(WorkspaceSnapshot {
        cwd: cwd.display().to_string(),
        root,
        repo,
        branch,
        base_sha,
    })
}

pub async fn changed_files(root: &str, base: Option<&str>) -> Result<Vec<ChangedFile>> {
    let root_path = Path::new(root);
    let base = base.unwrap_or("HEAD");
    let numstat = git(root_path, &["diff", "--numstat", base, "--"]).await?;
    let names = git(root_path, &["diff", "--name-status", base, "--"]).await?;
    let mut out = Vec::new();
    for line in names.lines() {
        let mut cols = line.split('\t');
        let status = cols.next().unwrap_or("").to_owned();
        let path = cols.last().unwrap_or("").to_owned();
        if path.is_empty() {
            continue;
        }
        let mut additions = 0;
        let mut deletions = 0;
        for stat in numstat.lines() {
            let parts: Vec<&str> = stat.split('\t').collect();
            if parts.last() == Some(&path.as_str()) {
                additions = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                deletions = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                break;
            }
        }
        out.push(ChangedFile {
            path,
            status,
            additions,
            deletions,
        });
    }
    let untracked = git(root_path, &["ls-files", "--others", "--exclude-standard"]).await?;
    for path in untracked.lines().filter(|s| !s.is_empty()) {
        out.push(ChangedFile {
            path: path.to_owned(),
            status: "??".to_owned(),
            additions: 0,
            deletions: 0,
        });
    }
    Ok(out)
}

pub async fn diff(root: &str, base: Option<&str>, path: Option<&str>) -> Result<String> {
    let root_path = Path::new(root);
    let base = base.unwrap_or("HEAD");
    match path.filter(|s| !s.is_empty()) {
        Some(path) => {
            let tracked = git(root_path, &["diff", "--find-renames", base, "--", path]).await?;
            if !tracked.is_empty() {
                return Ok(tracked);
            }
            let untracked = git(
                root_path,
                &["ls-files", "--others", "--exclude-standard", "--", path],
            )
            .await?;
            if untracked.lines().any(|p| p == path) {
                git_diff(root_path, &["diff", "--no-index", "--", "/dev/null", path]).await
            } else {
                Ok(String::new())
            }
        }
        None => {
            let mut out = git(root_path, &["diff", "--find-renames", base, "--"]).await?;
            let untracked = git(root_path, &["ls-files", "--others", "--exclude-standard"]).await?;
            for path in untracked.lines().filter(|s| !s.is_empty()) {
                let patch =
                    git_diff(root_path, &["diff", "--no-index", "--", "/dev/null", path]).await?;
                if !patch.is_empty() {
                    if !out.is_empty() {
                        out.push_str("\n");
                    }
                    out.push_str(&patch);
                }
            }
            Ok(out)
        }
    }
}

pub fn list_files(root: &str, dir: Option<&str>) -> Result<Vec<FileEntry>> {
    let root_path = Path::new(root).canonicalize()?;
    let rel = dir.unwrap_or("").trim_matches('/');
    let target = root_path
        .join(rel)
        .canonicalize()
        .unwrap_or(root_path.join(rel));
    if !target.starts_with(&root_path) {
        anyhow::bail!("path escapes workspace");
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&target).with_context(|| format!("read {}", target.display()))? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            continue;
        }
        let rel_path = path
            .strip_prefix(&root_path)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        entries.push(FileEntry {
            path: rel_path,
            name,
            is_dir: path.is_dir(),
        });
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    Ok(entries)
}

pub fn read_file(root: &str, path: &str) -> Result<FileContents> {
    const MAX_BYTES: usize = 512 * 1024;

    let root_path = Path::new(root).canonicalize()?;
    let rel = path.trim_matches('/');
    let target = root_path.join(rel).canonicalize()?;
    if !target.starts_with(&root_path) {
        anyhow::bail!("path escapes workspace");
    }
    if target.is_dir() {
        anyhow::bail!("path is a directory");
    }

    let bytes = std::fs::read(&target).with_context(|| format!("read {}", target.display()))?;
    let truncated = bytes.len() > MAX_BYTES;
    let slice = if truncated {
        &bytes[..MAX_BYTES]
    } else {
        &bytes[..]
    };
    let contents = String::from_utf8_lossy(slice).to_string();
    Ok(FileContents {
        contents,
        truncated,
    })
}
