//! The verbs. Activation is a pure function of manifest + recorded state:
//! ephemeral files are reseeded (drift archived to the journal first),
//! durable files fast-forward when clean and *stage* the incoming base when
//! drifted — nothing is ever auto-merged and nothing is ever lost. The rest
//! of the verbs operate on the queue that gating produces.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::diff::{self, Op};
use crate::store::{Meta, Persistence, Store, write_creating_parents};
use crate::value::{self, Format};

/// What `activate` receives from the nix module: the full set of declared
/// mutable files for this generation.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub files: Vec<ManifestEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    /// Target path; `~/` is expanded against `HOME`.
    pub path: String,
    /// Store path holding the declared content (the incoming base).
    pub source: String,
    /// Omitted ⇒ auto-detected from the target name and base contents,
    /// then sticky for the file's lifetime.
    #[serde(default)]
    pub format: Option<Format>,
    /// Omitted ⇒ ephemeral: edits are scratch and reset at activation/login.
    #[serde(default)]
    pub persistence: Option<Persistence>,
    #[serde(default)]
    pub declared_at: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
}

/// Reconciliation outcome for one file, printed as the activation log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Seeded,
    Reseeded,
    FastForwarded,
    AlreadyClean,
    DriftKept,
    Staged,
    Adopted,
}

impl Outcome {
    const fn label(self) -> &'static str {
        match self {
            Self::Seeded => "seeded",
            Self::Reseeded => "reseeded (drift archived to journal)",
            Self::FastForwarded => "updated",
            Self::AlreadyClean => "clean",
            Self::DriftKept => "drifted (base unchanged, edits kept)",
            Self::Staged => "conflict (incoming base staged, edits kept)",
            Self::Adopted => "clean (your edits match the incoming base)",
        }
    }
}

pub fn activate(store: &Store, manifest_path: &Path) -> Result<()> {
    let text = fs::read_to_string(manifest_path)
        .with_context(|| format!("reading manifest {}", manifest_path.display()))?;
    let manifest: Manifest = serde_json::from_str(&text)
        .with_context(|| format!("parsing manifest {}", manifest_path.display()))?;

    let mut declared = Vec::new();
    for entry in &manifest.files {
        let target = expand_tilde(&entry.path)?;
        declared.push(target.clone());
        let outcome =
            reconcile(store, entry, &target).with_context(|| format!("reconciling {target}"))?;
        println!("{} {target}", outcome.label());
    }

    // Files no longer declared: forget their state, leave them on disk.
    for meta in store.all_metas()? {
        if !declared.contains(&meta.path) {
            store.journal(&meta.path, "unmanaged", None)?;
            store.forget(&meta.path)?;
            println!("unmanaged {} (file left on disk)", meta.path);
        }
    }
    Ok(())
}

fn reconcile(store: &Store, entry: &ManifestEntry, target: &str) -> Result<Outcome> {
    let incoming =
        fs::read(&entry.source).with_context(|| format!("reading source {}", entry.source))?;
    let existing = store.meta(target)?;

    // Format is sticky once recorded so detection can never flip under an
    // existing diff; an explicit manifest format always wins.
    let format = entry
        .format
        .or_else(|| existing.as_ref().map(|meta| meta.format));
    let format = format.unwrap_or_else(|| value::detect(Path::new(target), &incoming));
    let persistence = entry.persistence.unwrap_or(Persistence::Ephemeral);

    let mut meta = Meta {
        path: target.to_owned(),
        source: entry.source.clone(),
        staged_source: existing
            .as_ref()
            .and_then(|meta| meta.staged_source.clone()),
        source_file: entry.source_file.clone(),
        declared_at: entry.declared_at.clone(),
        format,
        persistence,
        snoozed: existing.as_ref().and_then(|meta| meta.snoozed.clone()),
    };

    let outcome = match existing {
        None => first_seed(store, &meta, &incoming)?,
        Some(_) => match persistence {
            Persistence::Ephemeral => reseed(store, &mut meta, &incoming, "activate")?,
            Persistence::Durable => gate(store, &mut meta, &incoming)?,
        },
    };
    store.save_meta(&meta)?;
    Ok(outcome)
}

fn first_seed(store: &Store, meta: &Meta, incoming: &[u8]) -> Result<Outcome> {
    let target = Path::new(&meta.path);
    let pre_existing = fs::read(target).ok();
    store.write_base(&meta.path, incoming)?;
    store.journal(&meta.path, "managed", None)?;
    match (meta.persistence, pre_existing) {
        // A durable target that already had content keeps it: the existing
        // file becomes day-one drift instead of being clobbered.
        (Persistence::Durable, Some(found))
            if !diff::logically_equal(meta.format, incoming, &found) =>
        {
            store.journal(
                &meta.path,
                "kept-existing",
                Some(serde_json::json!({
                    "yourEdits": diff::diff_bytes(meta.format, incoming, &found),
                })),
            )?;
            // The pre-existing target can still be a read-only store symlink
            // (a home-manager link left by a `home.file` -> `mutable.files`
            // migration). Keeping its content as day-one drift is pointless
            // unless the file is writable, so materialize the symlink into a
            // plain file holding the same bytes — the one seeding path that
            // otherwise skips write_creating_parents' symlink replacement.
            if fs::symlink_metadata(target).is_ok_and(|md| md.file_type().is_symlink()) {
                write_creating_parents(target, &found)?;
            }
            Ok(Outcome::DriftKept)
        }
        (_, pre_existing) => {
            // Ephemeral (or logically-equal) pre-existing content is about
            // to be overwritten: archive its diff first so nothing is lost.
            if let Some(found) = pre_existing {
                let archived = diff::diff_bytes(meta.format, incoming, &found);
                if !archived.is_empty() {
                    store.journal(
                        &meta.path,
                        "reseeded",
                        Some(serde_json::json!({ "cause": "first-seed", "archived": archived })),
                    )?;
                }
            }
            write_creating_parents(target, incoming)?;
            Ok(Outcome::Seeded)
        }
    }
}

/// Ephemeral files: archive any drift's logical diff, then Nix wins.
fn reseed(store: &Store, meta: &mut Meta, incoming: &[u8], cause: &str) -> Result<Outcome> {
    let target = Path::new(&meta.path);
    let base = store.base_bytes(&meta.path)?;
    let upper = fs::read(target).unwrap_or_default();
    let drift = diff::diff_bytes(meta.format, &base, &upper);
    if drift.is_empty() && incoming == base.as_slice() && target.exists() {
        return Ok(Outcome::AlreadyClean);
    }
    if !drift.is_empty() {
        store.journal(
            &meta.path,
            "reseeded",
            Some(serde_json::json!({ "cause": cause, "archived": drift })),
        )?;
        meta.snoozed = None;
    }
    write_creating_parents(target, incoming)?;
    store.write_base(&meta.path, incoming)?;
    store.clear_staged(&meta.path)?;
    meta.staged_source = None;
    if drift.is_empty() {
        Ok(Outcome::FastForwarded)
    } else {
        Ok(Outcome::Reseeded)
    }
}

/// Durable files: the gated switch.
fn gate(store: &Store, meta: &mut Meta, incoming: &[u8]) -> Result<Outcome> {
    let target = Path::new(&meta.path);
    let base = store.base_bytes(&meta.path)?;
    // A deleted target is almost always accidental for a declared file:
    // recreate rather than treating deletion as drift.
    let Ok(upper) = fs::read(target) else {
        write_creating_parents(target, incoming)?;
        adopt_base(store, meta, incoming)?;
        store.journal(&meta.path, "seeded", None)?;
        return Ok(Outcome::Seeded);
    };

    if diff::logically_equal(meta.format, incoming, &upper) {
        // Your edits already match the incoming content (typically because
        // drift was absorbed into the repo). The diff being empty IS the
        // resolution — keep the upper's bytes, adopt the base.
        let outcome = if diff::logically_equal(meta.format, &base, &upper) {
            Outcome::AlreadyClean
        } else {
            Outcome::Adopted
        };
        adopt_base(store, meta, incoming)?;
        return Ok(outcome);
    }
    if diff::logically_equal(meta.format, &base, &upper) {
        // Clean under the old base: fast-forward to the new one.
        write_creating_parents(target, incoming)?;
        adopt_base(store, meta, incoming)?;
        store.journal(&meta.path, "base-updated", None)?;
        return Ok(Outcome::FastForwarded);
    }
    if diff::logically_equal(meta.format, &base, incoming) {
        // Base unchanged; your drift simply persists.
        return Ok(Outcome::DriftKept);
    }
    // Drifted AND the base moved: stage the incoming base, touch nothing.
    store.write_staged(&meta.path, incoming)?;
    meta.staged_source = Some(meta.source.clone());
    store.journal(&meta.path, "staged", None)?;
    Ok(Outcome::Staged)
}

fn adopt_base(store: &Store, meta: &mut Meta, incoming: &[u8]) -> Result<()> {
    store.write_base(&meta.path, incoming)?;
    store.clear_staged(&meta.path)?;
    meta.staged_source = None;
    meta.snoozed = None;
    Ok(())
}

/// The login/boot reseed for ephemeral files. Needs no manifest: bases are
/// already snapshotted in the store.
pub fn reseed_ephemeral(store: &Store) -> Result<()> {
    for mut meta in store.all_metas()? {
        if meta.persistence != Persistence::Ephemeral {
            continue;
        }
        let base = store.base_bytes(&meta.path)?;
        let outcome = reseed(store, &mut meta, &base, "reseed")?;
        store.save_meta(&meta)?;
        println!("{} {}", outcome.label(), meta.path);
    }
    Ok(())
}

// --- status ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Clean,
    Drifted,
    Conflict,
    Snoozed,
}

/// Everything the TUI needs to render one pending file: identity, the raw
/// texts to diff (base vs the file on disk, plus the staged incoming base
/// while a conflict is unresolved), and the conflict overlap.
pub struct TuiEntry {
    pub path: String,
    pub state: State,
    pub format: Format,
    pub persistence: Persistence,
    pub declared_at: Option<String>,
    pub base: String,
    pub upper: String,
    pub staged: Option<String>,
    pub overlap: Vec<String>,
}

pub fn tui_entries(store: &Store) -> Result<Vec<TuiEntry>> {
    let mut entries = Vec::new();
    for meta in store.all_metas()? {
        let entry = status_entry(store, &meta)?;
        if entry.state == State::Clean {
            continue;
        }
        let base = store.base_bytes(&meta.path)?;
        let upper = fs::read(&meta.path).unwrap_or_default();
        let staged = store.staged_bytes(&meta.path)?;
        entries.push(TuiEntry {
            path: entry.path,
            state: entry.state,
            format: meta.format,
            persistence: meta.persistence,
            declared_at: meta.declared_at.clone(),
            base: String::from_utf8_lossy(&base).into_owned(),
            upper: String::from_utf8_lossy(&upper).into_owned(),
            staged: staged.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            overlap: entry.overlap,
        });
    }
    Ok(entries)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusEntry {
    path: String,
    state: State,
    persistence: Persistence,
    #[serde(skip_serializing_if = "Option::is_none")]
    resets_at: Option<&'static str>,
    format: Format,
    #[serde(skip_serializing_if = "Option::is_none")]
    declared_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_file: Option<String>,
    base: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    staged_base: Option<String>,
    upper: String,
    your_edits: Vec<Op>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    incoming_edits: Vec<Op>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    overlap: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusReport {
    pending: Vec<StatusEntry>,
    clean: Vec<String>,
    snoozed: Vec<String>,
}

fn status_entry(store: &Store, meta: &Meta) -> Result<StatusEntry> {
    let base = store.base_bytes(&meta.path)?;
    let upper = fs::read(&meta.path).unwrap_or_default();
    let staged = store.staged_bytes(&meta.path)?;
    let your_edits = diff::diff_bytes(meta.format, &base, &upper);
    let incoming_edits = staged.as_ref().map_or_else(Vec::new, |bytes| {
        diff::diff_bytes(meta.format, &base, bytes)
    });
    let overlap = diff::overlap(meta.format, &your_edits, &incoming_edits);
    let state = if staged.is_some() {
        State::Conflict
    } else if your_edits.is_empty() {
        State::Clean
    } else if meta.snoozed.as_deref() == Some(diff::fingerprint(&your_edits).as_str()) {
        State::Snoozed
    } else {
        State::Drifted
    };
    Ok(StatusEntry {
        path: meta.path.clone(),
        state,
        persistence: meta.persistence,
        resets_at: matches!(meta.persistence, Persistence::Ephemeral).then_some("next-login"),
        format: meta.format,
        declared_at: meta.declared_at.clone(),
        source_file: meta.source_file.clone(),
        base: meta.source.clone(),
        staged_base: meta.staged_source.clone(),
        upper: meta.path.clone(),
        your_edits,
        incoming_edits,
        overlap,
    })
}

pub fn status(store: &Store, json: bool) -> Result<()> {
    let mut pending = Vec::new();
    let mut clean = Vec::new();
    let mut snoozed = Vec::new();
    for meta in store.all_metas()? {
        let entry = status_entry(store, &meta)?;
        match entry.state {
            State::Clean => clean.push(entry.path),
            State::Snoozed => snoozed.push(entry.path),
            State::Drifted | State::Conflict => pending.push(entry),
        }
    }
    if json {
        let report = StatusReport {
            pending,
            clean,
            snoozed,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for entry in &pending {
        let mark = if entry.state == State::Conflict {
            "✗ conflict"
        } else {
            "~ drifted "
        };
        println!(
            "{mark} {} ({} ops, {})",
            entry.path,
            entry.your_edits.len(),
            entry.format
        );
    }
    for path in &snoozed {
        println!("· snoozed  {path}");
    }
    for path in &clean {
        println!("✓ clean    {path}");
    }
    if pending.is_empty() {
        println!("nothing to resolve");
    }
    Ok(())
}

// --- per-file verbs ---

fn require_meta(store: &Store, path: &str) -> Result<Meta> {
    let target = expand_tilde(path)?;
    store
        .meta(&target)?
        .with_context(|| format!("{target} is not a managed mutable file"))
}

pub fn print_diff(store: &Store, path: &str, raw: bool) -> Result<()> {
    let meta = require_meta(store, path)?;
    let base = store.base_bytes(&meta.path)?;
    let upper = fs::read(&meta.path).unwrap_or_default();
    if raw {
        let old = String::from_utf8_lossy(&base);
        let new = String::from_utf8_lossy(&upper);
        print!(
            "{}",
            similar::TextDiff::from_lines(old.as_ref(), new.as_ref())
                .unified_diff()
                .context_radius(3)
                .header("base", "upper")
        );
        return Ok(());
    }
    let ops = diff::diff_bytes(meta.format, &base, &upper);
    println!("{}", serde_json::to_string_pretty(&ops)?);
    Ok(())
}

/// Drop your edits: the (staged, if any) base wins and the file goes clean.
pub fn discard(store: &Store, path: &str) -> Result<()> {
    let mut meta = require_meta(store, path)?;
    let winning = match store.staged_bytes(&meta.path)? {
        Some(staged) => {
            meta.source = meta
                .staged_source
                .take()
                .unwrap_or_else(|| meta.source.clone());
            staged
        }
        None => store.base_bytes(&meta.path)?,
    };
    let upper = fs::read(&meta.path).unwrap_or_default();
    let dropped = diff::diff_bytes(meta.format, &store.base_bytes(&meta.path)?, &upper);
    store.journal(
        &meta.path,
        "discarded",
        Some(serde_json::json!({ "dropped": dropped })),
    )?;
    write_creating_parents(Path::new(&meta.path), &winning)?;
    adopt_base(store, &mut meta, &winning)?;
    store.save_meta(&meta)?;
    println!("discarded edits to {}", meta.path);
    Ok(())
}

/// Accept the staged base as the new base but keep your upper: the conflict
/// becomes plain drift, re-diffed against what the config now declares.
pub fn adopt(store: &Store, path: &str) -> Result<()> {
    let mut meta = require_meta(store, path)?;
    let staged = store
        .staged_bytes(&meta.path)?
        .with_context(|| format!("{} has no staged base (not in conflict)", meta.path))?;
    meta.source = meta
        .staged_source
        .take()
        .unwrap_or_else(|| meta.source.clone());
    store.write_base(&meta.path, &staged)?;
    store.clear_staged(&meta.path)?;
    meta.snoozed = None;
    store.journal(&meta.path, "adopted", None)?;
    store.save_meta(&meta)?;
    println!("adopted staged base for {} (your edits kept)", meta.path);
    Ok(())
}

/// Silence a drifted file until its diff changes.
pub fn snooze(store: &Store, path: &str) -> Result<()> {
    let mut meta = require_meta(store, path)?;
    let base = store.base_bytes(&meta.path)?;
    let upper = fs::read(&meta.path).unwrap_or_default();
    let ops = diff::diff_bytes(meta.format, &base, &upper);
    if ops.is_empty() {
        bail!("{} is clean; nothing to snooze", meta.path);
    }
    if store.staged_bytes(&meta.path)?.is_some() {
        bail!(
            "{} is in conflict; resolve it (discard/adopt) instead of snoozing",
            meta.path
        );
    }
    meta.snoozed = Some(diff::fingerprint(&ops));
    store.journal(&meta.path, "snoozed", None)?;
    store.save_meta(&meta)?;
    println!("snoozed {} until its diff changes", meta.path);
    Ok(())
}

pub fn apply_ops(path: &str, ops_source: &str, format: Option<Format>) -> Result<()> {
    let text = if ops_source == "-" {
        std::io::read_to_string(std::io::stdin()).context("reading ops from stdin")?
    } else {
        fs::read_to_string(ops_source).with_context(|| format!("reading ops file {ops_source}"))?
    };
    let ops: Vec<Op> = serde_json::from_str(&text).context("parsing ops JSON")?;
    let target = expand_tilde(path)?;
    let used = crate::apply::apply_to_file(Path::new(&target), format, &ops)?;
    println!("applied {} ops to {target} ({used})", ops.len());
    Ok(())
}

pub fn journal(store: &Store, path: Option<&str>, json: bool) -> Result<()> {
    let filter = path.map(expand_tilde).transpose()?;
    let entries: Vec<_> = store
        .journal_entries()?
        .into_iter()
        .filter(|entry| filter.as_ref().is_none_or(|wanted| &entry.path == wanted))
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    for entry in &entries {
        let detail = entry
            .detail
            .as_ref()
            .map_or_else(String::new, |extra| format!(" {extra}"));
        println!("{} {} {}{detail}", entry.ts, entry.event, entry.path);
    }
    if entries.is_empty() {
        println!("journal is empty");
    }
    Ok(())
}

pub fn expand_tilde(path: &str) -> Result<String> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var("HOME").context("HOME is not set; cannot expand ~")?;
        return Ok(format!("{home}/{rest}"));
    }
    Ok(path.to_owned())
}

/// Exit code for `status --check`: 0 when nothing is pending, 1 otherwise.
pub fn pending_count(store: &Store) -> Result<usize> {
    let mut count = 0;
    for meta in store.all_metas()? {
        let entry = status_entry(store, &meta)?;
        if matches!(entry.state, State::Drifted | State::Conflict) {
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        store: Store,
        target: String,
        source: std::path::PathBuf,
        manifest: std::path::PathBuf,
    }

    impl Fixture {
        fn new(persistence: &str) -> Self {
            let dir = tempfile::tempdir().expect("tempdir");
            let root = dir.path();
            let target = root.join("home/config.json");
            let source = root.join("store/config.json");
            fs::create_dir_all(source.parent().expect("parent")).expect("mkdir");
            let manifest = root.join("manifest.json");
            let store = Store::open(root.join("state")).expect("open");
            let fixture = Self {
                target: target.to_string_lossy().into_owned(),
                source,
                manifest,
                store,
                _dir: dir,
            };
            fixture.write_manifest(persistence);
            fixture
        }

        fn write_manifest(&self, persistence: &str) {
            let manifest = serde_json::json!({
                "files": [{
                    "path": self.target,
                    "source": self.source,
                    "persistence": persistence,
                    "declaredAt": "home/test.nix:1",
                }],
            });
            fs::write(&self.manifest, manifest.to_string()).expect("write manifest");
        }

        fn set_base(&self, contents: &str) {
            fs::write(&self.source, contents).expect("write source");
        }

        fn activate(&self) {
            activate(&self.store, &self.manifest).expect("activate");
        }

        fn target_contents(&self) -> String {
            fs::read_to_string(&self.target).expect("read target")
        }

        fn entry(&self) -> StatusEntry {
            let meta = self
                .store
                .meta(&self.target)
                .expect("meta")
                .expect("managed");
            status_entry(&self.store, &meta).expect("status")
        }
    }

    #[test]
    fn ephemeral_reseed_archives_drift() {
        let fixture = Fixture::new("ephemeral");
        fixture.set_base(r#"{"a": 1}"#);
        fixture.activate();
        assert_eq!(fixture.entry().state, State::Clean);

        fs::write(&fixture.target, r#"{"a": 2}"#).expect("drift");
        assert_eq!(fixture.entry().state, State::Drifted);

        fixture.activate();
        assert_eq!(fixture.target_contents(), r#"{"a": 1}"#);
        assert_eq!(fixture.entry().state, State::Clean);
        let archived = fixture
            .store
            .journal_entries()
            .expect("journal")
            .into_iter()
            .find(|entry| entry.event == "reseeded")
            .expect("reseed archived");
        let detail = archived.detail.expect("detail");
        assert!(
            detail["archived"]
                .as_array()
                .is_some_and(|ops| !ops.is_empty())
        );
    }

    #[test]
    fn durable_gate_matrix() {
        let fixture = Fixture::new("durable");
        fixture.set_base(r#"{"a": 1}"#);
        fixture.activate();
        assert_eq!(fixture.entry().state, State::Clean);

        // Base unchanged + local drift → drift persists across activation.
        fs::write(&fixture.target, r#"{"a": 1, "mine": true}"#).expect("drift");
        fixture.activate();
        assert_eq!(fixture.entry().state, State::Drifted);
        assert_eq!(fixture.target_contents(), r#"{"a": 1, "mine": true}"#);

        // Base changed + local drift → conflict with both diffs and no overlap.
        fixture.set_base(r#"{"a": 2}"#);
        fixture.activate();
        let entry = fixture.entry();
        assert_eq!(entry.state, State::Conflict);
        assert_eq!(fixture.target_contents(), r#"{"a": 1, "mine": true}"#);
        assert_eq!(entry.your_edits.len(), 1);
        assert_eq!(entry.incoming_edits.len(), 1);
        assert!(entry.overlap.is_empty());

        // adopt: staged base becomes base, drift remains vs the new base.
        adopt(&fixture.store, &fixture.target).expect("adopt");
        let entry = fixture.entry();
        assert_eq!(entry.state, State::Drifted);

        // discard: base wins, file is clean.
        discard(&fixture.store, &fixture.target).expect("discard");
        assert_eq!(fixture.entry().state, State::Clean);
        assert_eq!(fixture.target_contents(), r#"{"a": 2}"#);
    }

    #[test]
    fn durable_clean_fast_forwards() {
        let fixture = Fixture::new("durable");
        fixture.set_base(r#"{"a": 1}"#);
        fixture.activate();
        fixture.set_base(r#"{"a": 2}"#);
        fixture.activate();
        assert_eq!(fixture.target_contents(), r#"{"a": 2}"#);
        assert_eq!(fixture.entry().state, State::Clean);
    }

    #[test]
    fn absorbing_drift_into_the_repo_resolves_on_switch() {
        let fixture = Fixture::new("durable");
        fixture.set_base(r#"{"a": 1}"#);
        fixture.activate();
        fs::write(&fixture.target, r#"{"a": 1, "mine": true}"#).expect("drift");
        // The model absorbs the edit into the repo; the next generation's
        // base is logically equal to the upper → clean, no bookkeeping.
        fixture.set_base(r#"{"a": 1, "mine": true}"#);
        fixture.activate();
        assert_eq!(fixture.entry().state, State::Clean);
        // Upper bytes were kept, not rewritten.
        assert_eq!(fixture.target_contents(), r#"{"a": 1, "mine": true}"#);
    }

    #[test]
    fn formatting_only_drift_is_clean() {
        let fixture = Fixture::new("durable");
        fixture.set_base(r#"{"a": 1, "b": 2}"#);
        fixture.activate();
        fs::write(&fixture.target, "{\n  \"a\": 1,\n  \"b\": 2\n}\n").expect("reformat");
        assert_eq!(fixture.entry().state, State::Clean);
    }

    #[test]
    fn snooze_holds_until_diff_changes() {
        let fixture = Fixture::new("durable");
        fixture.set_base(r#"{"a": 1}"#);
        fixture.activate();
        fs::write(&fixture.target, r#"{"a": 3}"#).expect("drift");
        snooze(&fixture.store, &fixture.target).expect("snooze");
        assert_eq!(fixture.entry().state, State::Snoozed);
        fs::write(&fixture.target, r#"{"a": 4}"#).expect("more drift");
        assert_eq!(fixture.entry().state, State::Drifted);
    }

    #[test]
    fn deleted_durable_target_is_recreated() {
        let fixture = Fixture::new("durable");
        fixture.set_base(r#"{"a": 1}"#);
        fixture.activate();
        fs::remove_file(&fixture.target).expect("delete");
        fixture.activate();
        assert_eq!(fixture.target_contents(), r#"{"a": 1}"#);
    }

    #[test]
    fn undeclared_files_are_forgotten_but_kept_on_disk() {
        let fixture = Fixture::new("durable");
        fixture.set_base(r#"{"a": 1}"#);
        fixture.activate();
        fs::write(
            &fixture.manifest,
            serde_json::json!({ "files": [] }).to_string(),
        )
        .expect("empty manifest");
        fixture.activate();
        assert!(fixture.store.meta(&fixture.target).expect("meta").is_none());
        assert_eq!(fixture.target_contents(), r#"{"a": 1}"#);
    }

    #[test]
    fn pre_existing_durable_content_becomes_day_one_drift() {
        let fixture = Fixture::new("durable");
        fs::create_dir_all(Path::new(&fixture.target).parent().expect("parent")).expect("mkdir");
        fs::write(&fixture.target, r#"{"handmade": true}"#).expect("pre-existing");
        fixture.set_base(r#"{"a": 1}"#);
        fixture.activate();
        assert_eq!(fixture.target_contents(), r#"{"handmade": true}"#);
        assert_eq!(fixture.entry().state, State::Drifted);
    }

    #[test]
    fn pre_existing_durable_symlink_is_materialized_writable() {
        // Migrating `home.file` -> `mutable.files` can leave the target as a
        // read-only store symlink. Day-one drift must still be writable.
        let fixture = Fixture::new("durable");
        let target = Path::new(&fixture.target);
        fs::create_dir_all(target.parent().expect("parent")).expect("mkdir");
        let old_render = fixture
            .source
            .parent()
            .expect("store dir")
            .join("old-render.json");
        fs::write(&old_render, r#"{"handmade": true}"#).expect("old render");
        std::os::unix::fs::symlink(&old_render, target).expect("symlink");
        fixture.set_base(r#"{"a": 1}"#);
        fixture.activate();
        // The symlink is replaced by a writable regular file with its content.
        assert!(
            fs::symlink_metadata(target)
                .expect("meta")
                .file_type()
                .is_file(),
            "target should be a regular file, not a symlink"
        );
        assert_eq!(fixture.target_contents(), r#"{"handmade": true}"#);
        assert_eq!(fixture.entry().state, State::Drifted);
    }
}
