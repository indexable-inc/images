//! On-disk state: one directory per managed file (keyed by a hash of its
//! absolute target path), holding the last-adopted base snapshot, the staged
//! incoming base while a conflict is unresolved, and a `meta.json` record.
//! An append-only journal at the root archives every state transition —
//! including the logical diff of edits an ephemeral reseed wiped, so a tweak
//! lost to a reboot is recoverable.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::value::Format;

/// Whether edits to a managed file survive the next activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Persistence {
    /// The default: edits are scratch. Every activation (and the login
    /// reseed) rewrites the file from its base after archiving the drift's
    /// logical diff to the journal. Nix always wins; conflicts cannot exist.
    Ephemeral,
    /// Edits survive activations. A base change under local drift parks the
    /// incoming base as *staged* and queues the file for resolution instead
    /// of touching it.
    Durable,
}

/// Per-file record. Everything `status` reports that isn't computed from
/// file contents lives here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    /// Absolute target path (tilde already expanded).
    pub path: String,
    /// Store path the current base was adopted from, for display.
    pub source: String,
    /// Store path parked during a conflict, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged_source: Option<String>,
    /// Repo file holding the declared content — where "absorb into Nix"
    /// edits go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    /// Nix file (and line, when known) that declared this entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declared_at: Option<String>,
    pub format: Format,
    pub persistence: Persistence,
    /// Fingerprint of the drift being deliberately ignored; the file
    /// resurfaces as soon as its diff changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed: Option<String>,
}

/// One archived state transition.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalEntry {
    /// Unix seconds.
    pub ts: u64,
    pub path: String,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(root.join("files"))
            .with_context(|| format!("creating state dir {}", root.display()))?;
        Ok(Self { root })
    }

    /// Resolve the state root: explicit flag, then `INDEX_DELTA_STATE_DIR`,
    /// then `$XDG_STATE_HOME/index-delta`, then `~/.local/state/index-delta`.
    pub fn resolve_root(flag: Option<PathBuf>) -> Result<PathBuf> {
        if let Some(dir) = flag {
            return Ok(dir);
        }
        if let Some(dir) = env_dir("INDEX_DELTA_STATE_DIR") {
            return Ok(dir);
        }
        if let Some(dir) = env_dir("XDG_STATE_HOME") {
            return Ok(dir.join("index-delta"));
        }
        let home = env_dir("HOME").context("HOME is not set; pass --state-dir")?;
        Ok(home.join(".local/state/index-delta"))
    }

    fn dir_for(&self, target: &str) -> PathBuf {
        let digest = Sha256::digest(target.as_bytes());
        self.root.join("files").join(&hex(&digest)[..16])
    }

    pub fn meta(&self, target: &str) -> Result<Option<Meta>> {
        let path = self.dir_for(target).join("meta.json");
        if !path.exists() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let meta =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(meta))
    }

    pub fn save_meta(&self, meta: &Meta) -> Result<()> {
        let dir = self.dir_for(&meta.path);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let text = serde_json::to_string_pretty(meta).context("serializing meta")?;
        fs::write(dir.join("meta.json"), text + "\n").context("writing meta.json")?;
        Ok(())
    }

    /// Every managed file's record, sorted by target path for stable output.
    pub fn all_metas(&self) -> Result<Vec<Meta>> {
        let files = self.root.join("files");
        let mut metas = Vec::new();
        for entry in fs::read_dir(&files).with_context(|| format!("listing {}", files.display()))? {
            let meta_path = entry?.path().join("meta.json");
            if !meta_path.exists() {
                continue;
            }
            let text = fs::read_to_string(&meta_path)
                .with_context(|| format!("reading {}", meta_path.display()))?;
            metas.push(
                serde_json::from_str(&text)
                    .with_context(|| format!("parsing {}", meta_path.display()))?,
            );
        }
        metas.sort_by(|a: &Meta, b: &Meta| a.path.cmp(&b.path));
        Ok(metas)
    }

    pub fn base_bytes(&self, target: &str) -> Result<Vec<u8>> {
        let path = self.dir_for(target).join("base");
        fs::read(&path).with_context(|| format!("reading base snapshot {}", path.display()))
    }

    pub fn write_base(&self, target: &str, bytes: &[u8]) -> Result<()> {
        let dir = self.dir_for(target);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        fs::write(dir.join("base"), bytes).context("writing base snapshot")?;
        Ok(())
    }

    pub fn staged_bytes(&self, target: &str) -> Result<Option<Vec<u8>>> {
        let path = self.dir_for(target).join("staged");
        if !path.exists() {
            return Ok(None);
        }
        let bytes =
            fs::read(&path).with_context(|| format!("reading staged base {}", path.display()))?;
        Ok(Some(bytes))
    }

    pub fn write_staged(&self, target: &str, bytes: &[u8]) -> Result<()> {
        fs::write(self.dir_for(target).join("staged"), bytes).context("writing staged base")?;
        Ok(())
    }

    pub fn clear_staged(&self, target: &str) -> Result<()> {
        let path = self.dir_for(target).join("staged");
        if path.exists() {
            fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
        Ok(())
    }

    /// Forget a file: drop its state directory. The file itself is left on
    /// disk — unmanaging is not deletion.
    pub fn forget(&self, target: &str) -> Result<()> {
        let dir = self.dir_for(target);
        if dir.exists() {
            fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
        }
        Ok(())
    }

    pub fn journal(&self, path: &str, event: &str, detail: Option<Value>) -> Result<()> {
        let entry = JournalEntry {
            ts: now_secs(),
            path: path.to_owned(),
            event: event.to_owned(),
            detail,
        };
        let mut line = serde_json::to_string(&entry).context("serializing journal entry")?;
        line.push('\n');
        let journal = self.root.join("journal.jsonl");
        let mut file = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&journal)
            .with_context(|| format!("opening {}", journal.display()))?;
        std::io::Write::write_all(&mut file, line.as_bytes())
            .with_context(|| format!("appending to {}", journal.display()))?;
        Ok(())
    }

    pub fn journal_entries(&self) -> Result<Vec<JournalEntry>> {
        let journal = self.root.join("journal.jsonl");
        if !journal.exists() {
            return Ok(Vec::new());
        }
        let text = fs::read_to_string(&journal)
            .with_context(|| format!("reading {}", journal.display()))?;
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line).with_context(|| format!("parsing journal line {line}"))
            })
            .collect()
    }
}

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Write a file, creating parent directories. Used for both seeding targets
/// and `apply-ops` output.
pub fn write_creating_parents(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dirs for {}", path.display()))?;
    }
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encodes() {
        assert_eq!(hex(&[0x00, 0xff, 0x10]), "00ff10");
    }

    #[test]
    fn meta_round_trips_and_journal_appends() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(dir.path().to_path_buf()).expect("open");
        let meta = Meta {
            path: "/tmp/x/config.json".to_owned(),
            source: "/nix/store/abc".to_owned(),
            staged_source: None,
            source_file: None,
            declared_at: Some("home/x.nix:3".to_owned()),
            format: Format::Json,
            persistence: Persistence::Ephemeral,
            snoozed: None,
        };
        store.save_meta(&meta).expect("save");
        let loaded = store.meta(&meta.path).expect("load").expect("present");
        assert_eq!(loaded.source, meta.source);
        assert!(store.meta("/tmp/other").expect("load").is_none());

        store.journal(&meta.path, "managed", None).expect("journal");
        store
            .journal(
                &meta.path,
                "reseeded",
                Some(serde_json::json!({"archived": []})),
            )
            .expect("journal");
        let entries = store.journal_entries().expect("entries");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].event, "reseeded");

        store.forget(&meta.path).expect("forget");
        assert!(store.meta(&meta.path).expect("load").is_none());
    }
}
