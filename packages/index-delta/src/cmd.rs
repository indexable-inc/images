//! The verbs. Activation is a pure function of manifest + recorded state:
//! ephemeral files are reseeded (drift archived to the journal first),
//! durable files fast-forward when clean and *stage* the incoming base when
//! drifted — nothing is ever auto-merged and nothing is ever lost. The rest
//! of the verbs operate on the queue that gating produces.

use std::collections::HashMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::str;

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
    let _lock = store.lock()?;
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
    let _lock = store.lock()?;
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

/// One side of a pending file's diff, in the representation its format is
/// modeled in. Structured formats diff as logical ops: object key order is
/// not significant (RFC 8259 §4, and likewise toml/yaml/plist tables), so a
/// byte-level line diff would bury one real edit under an app rewriting the
/// file in its own key order. Only true text files diff as lines.
pub enum TuiDiff {
    Text { old: String, new: String },
    Ops(Vec<Op>),
}

fn tui_diff(format: Format, old: &[u8], new: &[u8]) -> TuiDiff {
    match (format, str::from_utf8(old), str::from_utf8(new)) {
        (Format::Text, Ok(old), Ok(new)) => TuiDiff::Text {
            old: old.to_owned(),
            new: new.to_owned(),
        },
        // Structured formats, plus non-UTF-8 text sides (a binary plist,
        // say) where a lossy line diff would be gibberish.
        _ => TuiDiff::Ops(diff::diff_bytes(format, old, new)),
    }
}

/// Everything the TUI needs to render one pending file: identity, the diff
/// of each side (base vs the file on disk, plus base vs the staged incoming
/// base while a conflict is unresolved), and the conflict overlap.
// clone:ignore -- identifier-blind shape match with cve-scan's unrelated
// PackageEvidence (any two eight-field pub structs collide).
pub struct TuiEntry {
    pub path: String,
    pub state: State,
    pub format: Format,
    pub persistence: Persistence,
    pub declared_at: Option<String>,
    pub yours: TuiDiff,
    pub incoming: Option<TuiDiff>,
    pub overlap: Vec<String>,
}

pub fn tui_entries(store: &Store) -> Result<Vec<TuiEntry>> {
    let mut entries = Vec::new();
    for meta in store.all_metas()? {
        let entry = status_entry(store, &meta)?;
        let base = store.base_bytes(&meta.path)?;
        let upper = fs::read(&meta.path).unwrap_or_default();
        let staged = store.staged_bytes(&meta.path)?;
        entries.push(TuiEntry {
            path: entry.path,
            state: entry.state,
            format: meta.format,
            persistence: meta.persistence,
            declared_at: meta.declared_at.clone(),
            yours: tui_diff(meta.format, &base, &upper),
            incoming: staged.map(|staged| tui_diff(meta.format, &base, &staged)),
            overlap: entry.overlap,
        });
    }
    // Every managed file is listed; the ones needing attention sort first
    // and clean files sink to the bottom (stable, so store order holds
    // within a state).
    entries.sort_by_key(|entry| attention_rank(entry.state));
    Ok(entries)
}

const fn attention_rank(state: State) -> u8 {
    match state {
        State::Conflict => 0,
        State::Drifted => 1,
        State::Snoozed => 2,
        State::Clean => 3,
    }
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
    let _lock = store.lock()?;
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
    let _lock = store.lock()?;
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
    let _lock = store.lock()?;
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullReport {
    pulled: Vec<PulledFile>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PulledFile {
    path: String,
    source_file: String,
}

struct PullPlan {
    path: String,
    source_file: String,
    source_path: PathBuf,
    base: Vec<u8>,
    upper: Vec<u8>,
}

struct StagedPull {
    plan: PullPlan,
    output: tempfile::NamedTempFile,
    rollback: tempfile::NamedTempFile,
}

/// Copy selected text uppers into their recorded repository sources. All
/// candidates are validated before the first write so one stale source cannot
/// leave a bulk pull half applied.
pub fn pull(store: &Store, repo_root: &Path, path: Option<&str>) -> Result<PullReport> {
    let _lock = store.lock()?;
    let repo_root = repo_root
        .canonicalize()
        .with_context(|| format!("resolving repository root {}", repo_root.display()))?;
    if !repo_root.join(".git").exists() {
        bail!("{} is not a Git repository root", repo_root.display());
    }
    let metas = match path {
        Some(path) => vec![require_meta(store, path)?],
        None => store.all_metas()?,
    };
    let explicit = path.is_some();
    let mut plans = Vec::new();
    let mut destinations = HashMap::new();

    for meta in metas {
        if meta.format != Format::Text {
            if explicit {
                bail!("{} is tracked as {}, not text", meta.path, meta.format);
            }
            continue;
        }
        let Some(source_file) = meta.source_file.clone() else {
            if explicit {
                bail!("{} does not record a sourceFile", meta.path);
            }
            continue;
        };
        let base = store.base_bytes(&meta.path)?;
        let upper = fs::read(&meta.path)
            .with_context(|| format!("reading managed target {}", meta.path))?;
        if upper == base {
            continue;
        }
        if store.staged_bytes(&meta.path)?.is_some() {
            bail!(
                "{} has a staged conflict; resolve it before pulling",
                meta.path
            );
        }
        let relative = Path::new(&source_file);
        if relative.is_absolute() {
            bail!(
                "{} records an absolute sourceFile: {source_file}",
                meta.path
            );
        }
        let source_path = repo_root.join(relative).canonicalize().with_context(|| {
            format!(
                "resolving sourceFile {source_file} for managed target {}",
                meta.path
            )
        })?;
        if !source_path.starts_with(&repo_root) {
            bail!(
                "sourceFile {source_file} for {} escapes repository root {}",
                meta.path,
                repo_root.display()
            );
        }
        if let Some(other) = destinations.insert(source_path.clone(), meta.path.clone()) {
            bail!(
                "{} and {} record the same sourceFile destination: {source_file}",
                other,
                meta.path
            );
        }
        let repo_source = fs::read(&source_path)
            .with_context(|| format!("reading repository source {}", source_path.display()))?;
        if repo_source != base {
            bail!(
                "repository source {source_file} differs from the tracked base for {}; activate the current repository source before pulling",
                meta.path
            );
        }
        plans.push(PullPlan {
            path: meta.path,
            source_file,
            source_path,
            base,
            upper,
        });
    }

    let mut staged = Vec::with_capacity(plans.len());
    for plan in plans {
        let permissions = fs::metadata(&plan.source_path)
            .with_context(|| format!("reading metadata for {}", plan.source_path.display()))?
            .permissions();
        let output = stage_source(&plan.source_path, &plan.upper, permissions.clone())?;
        let rollback = stage_source(&plan.source_path, &plan.base, permissions)?;
        staged.push(StagedPull {
            plan,
            output,
            rollback,
        });
    }

    // The state lock keeps the tracked bases and staged-conflict markers fixed.
    // Recheck every source after staging before replacing any of them.
    for staged_file in &staged {
        let plan = &staged_file.plan;
        if store.staged_bytes(&plan.path)?.is_some() || store.base_bytes(&plan.path)? != plan.base {
            bail!(
                "{} changed in index-delta state while pull was staging",
                plan.path
            );
        }
        let current = fs::read(&plan.source_path).with_context(|| {
            format!(
                "rechecking repository source {}",
                plan.source_path.display()
            )
        })?;
        if current != plan.base {
            bail!(
                "repository source {} changed while pull was staging; no files were written",
                plan.source_file
            );
        }
    }

    let mut committed = Vec::with_capacity(staged.len());
    for staged_file in staged {
        let plan = &staged_file.plan;
        let current = fs::read(&plan.source_path).with_context(|| {
            format!(
                "rechecking repository source {}",
                plan.source_path.display()
            )
        })?;
        if current != plan.base {
            let error =
                anyhow::anyhow!("repository source {} changed during pull", plan.source_file);
            return Err(rollback_after_error(error, committed));
        }
        if let Err(error) = staged_file.output.persist(&plan.source_path) {
            let error = anyhow::Error::new(error.error).context(format!(
                "replacing repository source {} atomically",
                plan.source_path.display()
            ));
            return Err(rollback_after_error(error, committed));
        }
        committed.push((staged_file.plan, staged_file.rollback));
    }

    let mut pulled = Vec::with_capacity(committed.len());
    for (plan, _) in committed {
        pulled.push(PulledFile {
            path: plan.path,
            source_file: plan.source_file,
        });
    }
    Ok(PullReport { pulled })
}

fn stage_source(
    source_path: &Path,
    bytes: &[u8],
    permissions: fs::Permissions,
) -> Result<tempfile::NamedTempFile> {
    let parent = source_path
        .parent()
        .context("repository source has no parent directory")?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("staging repository source {}", source_path.display()))?;
    staged
        .as_file()
        .set_permissions(permissions)
        .with_context(|| format!("preserving permissions for {}", source_path.display()))?;
    staged
        .write_all(bytes)
        .with_context(|| format!("staging repository source {}", source_path.display()))?;
    staged
        .as_file()
        .sync_all()
        .with_context(|| format!("syncing repository source {}", source_path.display()))?;
    Ok(staged)
}

fn rollback_after_error(
    error: anyhow::Error,
    committed: Vec<(PullPlan, tempfile::NamedTempFile)>,
) -> anyhow::Error {
    let mut failed = Vec::new();
    for (plan, rollback) in committed {
        let still_ours = fs::read(&plan.source_path).is_ok_and(|current| current == plan.upper);
        if !still_ours || rollback.persist(&plan.source_path).is_err() {
            failed.push(plan.source_file);
        }
    }
    if failed.is_empty() {
        error.context("earlier repository writes were rolled back")
    } else {
        error.context(format!(
            "rollback failed for repository sources: {}",
            failed.join(", ")
        ))
    }
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
        dir: tempfile::TempDir,
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
                dir,
            };
            fixture.write_manifest(persistence);
            fixture
        }

        fn write_manifest(&self, persistence: &str) {
            self.write_manifest_for(persistence, None, None);
        }

        fn write_manifest_for(
            &self,
            persistence: &str,
            format: Option<&str>,
            source_file: Option<&str>,
        ) {
            let manifest = serde_json::json!({
                "files": [{
                    "path": self.target,
                    "source": self.source,
                    "persistence": persistence,
                    "declaredAt": "home/test.nix:1",
                    "format": format,
                    "sourceFile": source_file,
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

        fn text_repo(&self) -> (PathBuf, PathBuf) {
            let repo = self.dir.path().join("repo");
            let repo_source = repo.join("config.nu");
            fs::create_dir_all(&repo).expect("mkdir repo");
            fs::write(repo.join(".git"), "gitdir: test\n").expect("mark repo root");
            fs::write(&repo_source, "let value = 1\n").expect("write repo source");
            self.set_base("let value = 1\n");
            self.write_manifest_for("durable", Some("text"), Some("config.nu"));
            (repo, repo_source)
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
    fn pull_copies_text_upper_to_recorded_source_and_reports_json() {
        let fixture = Fixture::new("durable");
        let (repo, repo_source) = fixture.text_repo();
        fixture.activate();
        fs::write(&fixture.target, "let value = 2\n").expect("drift");

        let report = pull(&fixture.store, &repo, Some(&fixture.target)).expect("pull");

        assert_eq!(
            fs::read_to_string(repo_source).expect("read repo source"),
            "let value = 2\n"
        );
        assert_eq!(
            serde_json::to_value(report).expect("serialize report"),
            serde_json::json!({
                "pulled": [{
                    "path": fixture.target,
                    "sourceFile": "config.nu",
                }],
            })
        );
    }

    #[test]
    fn pull_refuses_when_repository_source_moved_from_tracked_base() {
        let fixture = Fixture::new("durable");
        let (repo, repo_source) = fixture.text_repo();
        fixture.activate();
        fs::write(&fixture.target, "let value = 2\n").expect("drift");
        fs::write(&repo_source, "let value = 3\n").expect("move repo source");

        let error = pull(&fixture.store, &repo, Some(&fixture.target)).expect_err("reject stale");

        assert!(error.to_string().contains("differs from the tracked base"));
        assert_eq!(
            fs::read_to_string(repo_source).expect("read repo source"),
            "let value = 3\n"
        );
    }

    #[test]
    fn pull_refuses_a_staged_conflict() {
        let fixture = Fixture::new("durable");
        let (repo, repo_source) = fixture.text_repo();
        fixture.activate();
        fs::write(&fixture.target, "let local = true\n").expect("drift");
        fixture.set_base("let value = 2\n");
        fixture.activate();

        let error =
            pull(&fixture.store, &repo, Some(&fixture.target)).expect_err("reject conflict");

        assert!(error.to_string().contains("has a staged conflict"));
        assert_eq!(
            fs::read_to_string(repo_source).expect("read repo source"),
            "let value = 1\n"
        );
    }

    #[test]
    fn pull_refuses_a_source_outside_the_repository() {
        let fixture = Fixture::new("durable");
        let (repo, _) = fixture.text_repo();
        fixture.activate();
        fs::write(&fixture.target, "let value = 2\n").expect("drift");
        fs::write(fixture.dir.path().join("outside.nu"), "let value = 1\n")
            .expect("write outside source");
        let mut meta = fixture
            .store
            .meta(&fixture.target)
            .expect("meta")
            .expect("managed");
        meta.source_file = Some("../outside.nu".to_owned());
        fixture.store.save_meta(&meta).expect("save meta");

        let error = pull(&fixture.store, &repo, Some(&fixture.target)).expect_err("reject escape");

        assert!(error.to_string().contains("escapes repository root"));
    }

    #[test]
    fn pull_refuses_duplicate_repository_destinations() {
        let fixture = Fixture::new("durable");
        let (repo, repo_source) = fixture.text_repo();
        let other = fixture.dir.path().join("home/other.nu");
        let other_string = other.to_string_lossy().into_owned();
        let files = [&fixture.target, &other_string].map(|path| {
            serde_json::json!({
                "path": path,
                "source": fixture.source,
                "persistence": "durable",
                "format": "text",
                "sourceFile": "config.nu",
            })
        });
        fs::write(
            &fixture.manifest,
            serde_json::json!({ "files": files }).to_string(),
        )
        .expect("write manifest");
        fixture.activate();
        fs::write(&fixture.target, "let value = 2\n").expect("first drift");
        fs::write(&other, "let value = 3\n").expect("second drift");

        let error = pull(&fixture.store, &repo, None).expect_err("reject duplicate");

        assert!(error.to_string().contains("same sourceFile destination"));
        assert_eq!(
            fs::read_to_string(repo_source).expect("read repo source"),
            "let value = 1\n"
        );
    }

    #[test]
    fn pull_skips_an_explicit_clean_target() {
        let fixture = Fixture::new("durable");
        let (repo, repo_source) = fixture.text_repo();
        fixture.activate();

        let report = pull(&fixture.store, &repo, Some(&fixture.target)).expect("pull clean");

        assert_eq!(
            serde_json::to_value(report).expect("serialize report"),
            serde_json::json!({ "pulled": [] })
        );
        assert_eq!(
            fs::read_to_string(repo_source).expect("read repo source"),
            "let value = 1\n"
        );
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
    #[test]
    fn tui_diff_uses_logical_ops_for_binary_sides() {
        let bplist = |value: i64| {
            let mut dict = plist::Dictionary::new();
            dict.insert("a".to_owned(), plist::Value::from(value));
            let mut bytes = Vec::new();
            plist::Value::Dictionary(dict)
                .to_writer_binary(&mut bytes)
                .expect("serialize bplist");
            bytes
        };
        let (old, new) = (bplist(1), bplist(2));
        assert!(str::from_utf8(&old).is_err(), "fixture must be binary");
        let TuiDiff::Ops(ops) = tui_diff(Format::Plist, &old, &new) else {
            panic!("binary sides must diff as logical ops, not lossy text");
        };
        assert_eq!(
            ops,
            vec![Op::Replace {
                path: "/a".to_owned(),
                from: 1.into(),
                to: 2.into(),
            }]
        );
        assert!(matches!(
            tui_diff(Format::Text, b"a\n", b"b\n"),
            TuiDiff::Text { .. }
        ));
    }

    #[test]
    fn tui_lists_clean_files_after_pending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let store = Store::open(root.join("state")).expect("open");
        // "a" sorts before "b" by path; only attention rank may reorder them.
        let clean = root.join("home/a.json");
        let drifted = root.join("home/b.json");
        let source = root.join("store/base.json");
        fs::create_dir_all(source.parent().expect("parent")).expect("mkdir");
        fs::write(&source, r#"{"a": 1}"#).expect("write source");
        let manifest = root.join("manifest.json");
        let files: Vec<_> = [&clean, &drifted]
            .into_iter()
            .map(|path| {
                serde_json::json!({
                    "path": path, "source": source, "persistence": "durable",
                })
            })
            .collect();
        fs::write(&manifest, serde_json::json!({ "files": files }).to_string())
            .expect("write manifest");
        activate(&store, &manifest).expect("activate");
        fs::write(&drifted, r#"{"a": 2}"#).expect("drift");

        let entries = tui_entries(&store).expect("entries");
        let states: Vec<_> = entries
            .iter()
            .map(|entry| (entry.state, entry.path.clone()))
            .collect();
        assert_eq!(
            states,
            vec![
                (State::Drifted, drifted.to_string_lossy().into_owned()),
                (State::Clean, clean.to_string_lossy().into_owned()),
            ]
        );
    }

    #[test]
    fn tui_diff_hides_key_reorder_noise_in_structured_formats() {
        // An app rewriting a json file in its own key order must not drown
        // the one real edit: the model diff shows exactly that edit.
        let old = br#"{"a": 1, "src": {"repo": "r", "source": "github"}}"#;
        let new = br#"{"src": {"source": "github", "repo": "r"}, "a": 1, "model": "m"}"#;
        let TuiDiff::Ops(ops) = tui_diff(Format::Json, old, new) else {
            panic!("structured formats must diff as logical ops");
        };
        assert_eq!(
            ops,
            vec![Op::Add {
                path: "/model".to_owned(),
                value: "m".into(),
            }]
        );
    }
}
