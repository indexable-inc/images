//! Channel discovery and per-channel message loading.
//!
//! The export has one directory per channel, named either `C0...--name`,
//! `C0...-name`, or just `name` (no id prefix). The leading `C0...` token, when
//! present, is the channel id; otherwise the id is resolved from `channels.json`
//! by name. Each directory holds `YYYY-MM-DD.json` files, every one an array of
//! message objects. Replies of a thread can live in different day files, so a
//! caller that wants whole threads must read the whole directory.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use snafu::ResultExt as _;

use crate::{
    error::{Error, ListDirSnafu, ParseSnafu, ReadSnafu},
    model::{ChannelEntry, Message},
};

/// The resolved facts about one channel needed to build its thread documents.
#[derive(Debug, Clone)]
pub struct ChannelInfo {
    /// Stable channel id, e.g. `C0AGC8VFVQV`.
    pub id: String,
    /// Display name with any id prefix stripped, e.g. `alerts`.
    pub name: String,
    /// Whether the channel is archived.
    pub is_archived: bool,
    /// Whether this is an external / Slack-Connect channel (heuristic; see
    /// [`is_external_name`]).
    pub is_external: bool,
    /// Channel topic text, possibly empty.
    pub topic: String,
    /// Absolute path of the channel's directory.
    pub dir: PathBuf,
}

/// A directory entry whose name encodes (optionally) a channel id and a name.
#[derive(Debug, Clone)]
pub struct ChannelDir {
    /// Channel id parsed from the prefix, if the directory had one.
    pub id_from_prefix: Option<String>,
    /// The directory name with any leading `C0...` token removed.
    pub display_name: String,
    /// The full original directory name, used for the external heuristic.
    pub raw_dir_name: String,
    /// Absolute path of the directory.
    pub path: PathBuf,
}

/// Files at the export root that are not channel directories and must be skipped
/// when discovering channels.
const NON_CHANNEL_DIRS: &[&str] = &["__files__", "agent-traces"];

/// Discover every channel directory under the export root.
///
/// A subdirectory is treated as a channel unless it is a known auxiliary
/// directory. Auxiliary top-level JSON files are ignored because we only list
/// directories here.
///
/// # Errors
/// Returns [`Error::ListDir`] if the export root cannot be listed.
pub fn discover_channel_dirs(root: &Path) -> Result<Vec<ChannelDir>, Error> {
    let mut dirs = Vec::new();
    let entries = fs::read_dir(root).context(ListDirSnafu { path: root.to_path_buf() })?;
    for entry in entries {
        let entry = entry.context(ListDirSnafu { path: root.to_path_buf() })?;
        let file_type = entry.file_type().context(ListDirSnafu { path: root.to_path_buf() })?;
        if !file_type.is_dir() {
            continue;
        }
        let raw = entry.file_name().to_string_lossy().into_owned();
        if NON_CHANNEL_DIRS.contains(&raw.as_str()) {
            continue;
        }
        let (id_from_prefix, display_name) = split_id_prefix(&raw);
        dirs.push(ChannelDir {
            id_from_prefix,
            display_name,
            raw_dir_name: raw,
            path: entry.path(),
        });
    }
    dirs.sort_by(|a, b| a.raw_dir_name.cmp(&b.raw_dir_name));
    Ok(dirs)
}

/// Split a `C0...` id prefix from a directory name.
///
/// Recognizes `C0XYZ--name`, `C0XYZ-name`, and a bare `C0XYZ`. A name with no
/// id prefix returns `(None, name)`. The id token is a `C` followed by
/// uppercase alphanumerics.
#[must_use]
fn split_id_prefix(raw: &str) -> (Option<String>, String) {
    if !raw.starts_with('C') {
        return (None, raw.to_owned());
    }
    let id_len = raw
        .char_indices()
        .take_while(|(_, ch)| ch.is_ascii_uppercase() || ch.is_ascii_digit())
        .map(|(idx, ch)| idx + ch.len_utf8())
        .last()
        .unwrap_or(0);
    // A real id is more than just "C"; require at least a few chars.
    if id_len < 4 {
        return (None, raw.to_owned());
    }
    let (id, rest) = raw.split_at(id_len);
    let name = rest.trim_start_matches('-');
    if name.is_empty() {
        // Directory was just the id; use the id as the display name.
        (Some(id.to_owned()), id.to_owned())
    } else {
        (Some(id.to_owned()), name.to_owned())
    }
}

/// Heuristic for whether a channel is external / Slack-Connect.
///
/// Slack-Connect channels in this export are named with an `ext-` segment
/// (e.g. `ext-softmachine`, `C0...-ext-nu`). We flag a channel external when
/// its name (id prefix stripped) starts with `ext-` or contains `-ext-`. This
/// is a documented name heuristic, not a schema field: the export's
/// `channels.json` carries no `is_ext_shared`/`is_shared` flag.
#[must_use]
pub fn is_external_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("ext-") || lower.contains("-ext-") || lower == "ext"
}

/// Resolve a [`ChannelDir`] to a full [`ChannelInfo`], filling id, topic, and
/// archived flag from `channels.json` when available.
///
/// Resolution order for the id: the directory-name prefix, else a lookup by
/// display name in `channels.json`, else a synthetic id derived from the name
/// (so a channel with no metadata entry still gets a stable `external_id`).
#[must_use]
pub fn resolve_channel(dir: &ChannelDir, by_id: &HashMap<String, ChannelEntry>, by_name: &HashMap<String, ChannelEntry>) -> ChannelInfo {
    let entry = dir
        .id_from_prefix
        .as_deref()
        .and_then(|id| by_id.get(id))
        .or_else(|| by_name.get(&dir.display_name));

    let id = dir
        .id_from_prefix
        .clone()
        .or_else(|| entry.map(|entry| entry.id.clone()))
        .unwrap_or_else(|| format!("name:{}", dir.display_name));

    let name = dir.display_name.clone();
    let is_archived = entry.is_some_and(|entry| entry.is_archived);
    let topic = entry.map(|entry| entry.topic.value.clone()).unwrap_or_default();
    let is_external = is_external_name(&name) || is_external_name(&dir.raw_dir_name);

    ChannelInfo {
        id,
        name,
        is_archived,
        is_external,
        topic,
        dir: dir.path.clone(),
    }
}

/// Read and parse every `YYYY-MM-DD.json` day file in a channel directory into
/// a single flat list of messages.
///
/// Files are read in sorted name order for determinism, but ordering within a
/// thread is re-established later by ts, so the read order does not affect the
/// final body.
///
/// # Errors
/// Returns [`Error::ListDir`] / [`Error::Read`] / [`Error::Parse`] if the
/// directory cannot be listed or a day file cannot be read or parsed.
pub fn read_channel_messages(dir: &Path) -> Result<Vec<Message>, Error> {
    let mut day_files: Vec<PathBuf> = Vec::new();
    let entries = fs::read_dir(dir).context(ListDirSnafu { path: dir.to_path_buf() })?;
    for entry in entries {
        let entry = entry.context(ListDirSnafu { path: dir.to_path_buf() })?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            day_files.push(path);
        }
    }
    day_files.sort();

    let mut messages = Vec::new();
    for path in day_files {
        let bytes = fs::read(&path).context(ReadSnafu { path: path.clone() })?;
        let day: Vec<Message> = serde_json::from_slice(&bytes).context(ParseSnafu { path: path.clone() })?;
        messages.extend(day);
    }
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::{is_external_name, split_id_prefix};

    #[test]
    fn splits_double_dash_id_prefix() {
        let (id, name) = split_id_prefix("C0AGC8VFVQV--alerts");
        assert_eq!(id.as_deref(), Some("C0AGC8VFVQV"));
        assert_eq!(name, "alerts");
    }

    #[test]
    fn splits_single_dash_id_prefix() {
        let (id, name) = split_id_prefix("C0AQELZ5H3N-ext-nu");
        assert_eq!(id.as_deref(), Some("C0AQELZ5H3N"));
        assert_eq!(name, "ext-nu");
    }

    #[test]
    fn leaves_plain_name_alone() {
        let (id, name) = split_id_prefix("p-billing");
        assert_eq!(id, None);
        assert_eq!(name, "p-billing");
    }

    #[test]
    fn bare_id_directory_uses_id_as_name() {
        let (id, name) = split_id_prefix("C0AHW2W3V7W");
        assert_eq!(id.as_deref(), Some("C0AHW2W3V7W"));
        assert_eq!(name, "C0AHW2W3V7W");
    }

    #[test]
    fn external_heuristic() {
        assert!(is_external_name("ext-softmachine"));
        assert!(is_external_name("C0AQELZ5H3N-ext-nu"));
        assert!(!is_external_name("p-billing"));
        assert!(!is_external_name("context"));
    }
}
