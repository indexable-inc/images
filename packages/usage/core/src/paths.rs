//! Filesystem locations. Every accessor returns `None` when no home
//! directory exists (for example inside a nix build sandbox); callers treat
//! that as "telemetry is a no-op here".

use std::path::PathBuf;

/// State dir holding the spool and the `SQLite` database.
///
/// `IX_USAGE_STATE_DIR` overrides (tests, sandboxes), else
/// `$XDG_STATE_HOME/ix`, else `~/.local/state/ix`.
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("IX_USAGE_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Some(dir.join("ix"));
        }
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state/ix"))
}

/// The append-only ingress spool, folded into `SQLite` by [`crate::store::compact`].
#[must_use]
pub fn spool_path() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("usage.spool"))
}

/// The `SQLite` database (source of truth, agent-queryable).
#[must_use]
pub fn db_path() -> Option<PathBuf> {
    state_dir().map(|dir| dir.join("usage.db"))
}

/// The consent config file.
///
/// `IX_USAGE_CONFIG` overrides, else `$XDG_CONFIG_HOME/ix/usage.toml`, else
/// `~/.config/ix/usage.toml`.
#[must_use]
pub fn config_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("IX_USAGE_CONFIG") {
        return Some(PathBuf::from(path));
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        let dir = PathBuf::from(dir);
        if dir.is_absolute() {
            return Some(dir.join("ix/usage.toml"));
        }
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/ix/usage.toml"))
}
