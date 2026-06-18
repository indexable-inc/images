//! "What changed" explanations for root-cause derivations.
//!
//! The build stream says *that* a derivation rebuilt, never *why* -- and in Nix a
//! tiny input change silently rehashes everything downstream, so a rebuild often
//! looks unprovoked. For a root cause (its whole input closure is cache hits, so
//! it is the actual trigger) we diff its `.drv` against the *previous* build of
//! the same output and report what differs: "input rustc changed", "source
//! changed", etc. That is the answer to "why did Nix rebuild this".
//!
//! The baseline is the `.drv` that built the currently-installed output of the
//! same name (found via `nix-store --query --deriver`), so an explanation is
//! available on the very first run with no history file. The store is scanned
//! once and the name->paths index cached for the process.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use nix_web_monitor_parser::MonitorState;
use serde_json::Value;
use tokio::process::Command;
use tokio::sync::{OnceCell, RwLock, broadcast};

use crate::broadcast_deltas;

/// How many changed input names to name before summarising the rest as "+N more",
/// so a reason line stays scannable.
const MAX_NAMED: usize = 3;

/// Unix-seconds when the monitor started, recorded once at startup. Candidate
/// baselines registered at or after this are outputs produced by the *current*
/// run -- in particular the very build being explained, which for a
/// content-addressed derivation registers under its *resolved* deriver (not the
/// original drv the row is keyed by) and would otherwise be mistaken for the
/// previous build. Excluding them keeps the baseline a genuinely prior build.
static START_TIME: std::sync::OnceLock<i64> = std::sync::OnceLock::new();

/// Record the monitor's start time. Call once at startup, before any build runs.
pub fn record_start_time() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_secs()).ok())
        .unwrap_or(i64::MAX);
    let _ = START_TIME.set(now);
}

/// The start-time cutoff; `i64::MAX` (exclude nothing) when unset, e.g. in tests.
fn start_cutoff() -> i64 {
    START_TIME.get().copied().unwrap_or(i64::MAX)
}

/// Compute a "what changed" reason for one root-cause derivation and record it.
/// Best-effort: a query failure leaves the row unexplained rather than aborting.
pub async fn resolve_reason(
    derivation: String,
    monitor: &Arc<RwLock<MonitorState>>,
    deltas: &broadcast::Sender<Bytes>,
) -> Result<()> {
    let reason = match reason_for(&derivation).await {
        Ok(reason) => reason,
        Err(error) => {
            eprintln!("nix-web-monitor: reason query failed for {derivation}: {error:#}");
            return Ok(());
        }
    };
    monitor.write().await.set_rebuild_reason(derivation, reason);
    broadcast_deltas(monitor, deltas).await
}

/// The "what changed" explanation for `drv`: find the previous build of the same
/// output and diff the two `.drv`s.
async fn reason_for(drv: &str) -> Result<String> {
    let Some(name) = output_name(drv) else {
        return Ok("rebuilt".to_owned());
    };
    let Some(baseline) = previous_drv(drv, name).await? else {
        return Ok("no prior build to compare".to_owned());
    };
    diff_reason(drv, &baseline).await
}

/// The output name of a store derivation path: `/nix/store/<32-hash>-<name>.drv`
/// -> `<name>`. `None` if it does not have that shape. Shares the basename parse
/// with [`store_path_name`] (the `.drv` is just an extra suffix).
fn output_name(drv: &str) -> Option<&str> {
    store_path_name(drv.strip_suffix(".drv")?)
}

/// Whether a store path or basename names a derivation (ends in `.drv`). Uses the
/// path extension (case-insensitive) rather than a `.ends_with(".drv")` literal,
/// which clippy's pedantic lint rejects.
fn is_drv(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("drv"))
}

/// The `.drv` that built the previous build of the same `name` to diff against:
/// among installed outputs of that exact name, the deriver of the one *most
/// recently registered* in the store (excluding `current`). Picking the newest
/// (not an arbitrary match) makes the diff reflect the build you most likely had
/// before, when several old versions of a name are still in the store. `None`
/// when nothing else of that name is installed (a genuinely new derivation).
///
/// Uses a single batched `nix path-info` for all candidates: a frequently-built
/// name can have hundreds of same-name outputs in the store, so a per-candidate
/// query would spawn hundreds of subprocesses.
async fn previous_drv(current: &str, name: &str) -> Result<Option<String>> {
    let outputs = store_outputs_named(name).await?;
    if outputs.is_empty() {
        return Ok(None);
    }
    let cutoff = start_cutoff();
    Ok(path_infos(&outputs)
        .await?
        .into_iter()
        // `registered < cutoff` drops this-run outputs (e.g. the CA build's own
        // resolved-deriver output) so the baseline is a genuinely prior build.
        .filter(|candidate| {
            candidate.deriver != current && is_drv(&candidate.deriver) && candidate.registered < cutoff
        })
        .max_by_key(|candidate| candidate.registered)
        .map(|candidate| candidate.deriver))
}

/// One candidate baseline: a prior build's deriver `.drv` and its store
/// registration time (used to pick the newest).
struct Candidate {
    deriver: String,
    registered: i64,
}

/// A [`Candidate`] for each store path that records a deriver, from a single
/// `nix path-info --json` over all `paths` (one subprocess, not one per path).
/// Handles both the array and path-keyed-object shapes the command has used
/// across Nix versions; a failed query yields an empty list (best-effort:
/// produce no reason rather than error the build view).
async fn path_infos(paths: &[String]) -> Result<Vec<Candidate>> {
    let out = Command::new("nix")
        .args(["path-info", "--json"])
        .args(paths)
        .output()
        .await
        .context("spawning nix path-info")?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    let json: Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
    let entries: Vec<&Value> = match &json {
        Value::Array(items) => items.iter().collect(),
        Value::Object(map) => map.values().collect(),
        _ => return Ok(Vec::new()),
    };
    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let deriver = entry.get("deriver")?.as_str()?.to_owned();
            let registered = entry
                .get("registrationTime")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MIN);
            Some(Candidate { deriver, registered })
        })
        .collect())
}

/// Store output paths (non-`.drv`) whose output name is *exactly* `name`, from a
/// cached one-time scan of `/nix/store`. Exact-name (not suffix) match: many
/// names are dash-suffixes of others (`cargo-package-serde` vs `serde`), so a
/// suffix match would diff the root cause against an unrelated derivation. The
/// index is taken once and reused: it only needs the *pre-existing* outputs, so
/// a snapshot is correct.
async fn store_outputs_named(name: &str) -> Result<Vec<String>> {
    static INDEX: OnceCell<Vec<String>> = OnceCell::const_new();
    let entries = INDEX
        .get_or_try_init(|| async {
            let mut names = Vec::new();
            let mut dir = tokio::fs::read_dir("/nix/store")
                .await
                .context("reading /nix/store")?;
            while let Some(entry) = dir.next_entry().await.context("scanning /nix/store")? {
                if let Ok(name) = entry.file_name().into_string()
                    && !is_drv(&name)
                {
                    names.push(name);
                }
            }
            Ok::<_, anyhow::Error>(names)
        })
        .await?;
    Ok(entries
        .iter()
        .filter(|basename| store_path_name(basename) == Some(name))
        .map(|basename| format!("/nix/store/{basename}"))
        .collect())
}

/// Diff two derivations via `nix derivation show` and name what changed, in
/// priority order: a changed input derivation (the usual culprit, e.g. a bumped
/// `rustc`), then a changed source, then changed build env.
async fn diff_reason(current: &str, baseline: &str) -> Result<String> {
    let out = Command::new("nix")
        .args(["derivation", "show", current, baseline])
        .output()
        .await
        .context("spawning nix derivation show")?;
    if !out.status.success() {
        return Ok("rebuilt".to_owned());
    }
    let json: Value = serde_json::from_slice(&out.stdout).context("parsing derivation show")?;
    let obj = json.as_object().context("derivation show is not an object")?;
    // CA derivations can key by a resolved path, so when the requested key is
    // absent fall back to the *other* entry by key identity (not map-iteration
    // order, which could pick the baseline for both and compare it with itself).
    let cur = obj
        .get(current)
        .or_else(|| obj.iter().find(|(key, _)| key.as_str() != baseline).map(|(_, value)| value));
    let base = obj
        .get(baseline)
        .or_else(|| obj.iter().find(|(key, _)| key.as_str() != current).map(|(_, value)| value));
    let (Some(cur), Some(base)) = (cur, base) else {
        return Ok("rebuilt".to_owned());
    };

    let changed = changed_input_names(cur, base);
    if !changed.is_empty() {
        return Ok(format!("input {} changed", summarise(&changed)));
    }
    // Sources are compared by full store path (not name): the path encodes the
    // content hash, so a same-named source whose *content* changed (the common
    // case -- a project's own `src`) shows up as a different path here.
    if path_set(set_field(cur, "inputSrcs")) != path_set(set_field(base, "inputSrcs")) {
        return Ok("source changed".to_owned());
    }
    let env_keys = changed_env_keys(cur, base);
    if !env_keys.is_empty() {
        return Ok(format!("build env changed ({})", summarise(&env_keys)));
    }
    Ok("rebuilt (inputs identical; output was not cached)".to_owned())
}

/// Names of input derivations whose contributing `.drv` differs between the two
/// derivations -- the inputs that actually changed. Compared by output *name* so
/// a hash bump on the same logical input is what registers.
fn changed_input_names(cur: &Value, base: &Value) -> BTreeSet<String> {
    let cur_inputs = inputs_by_name(cur);
    let base_inputs = inputs_by_name(base);
    let mut changed = BTreeSet::new();
    for (name, drvs) in &cur_inputs {
        if base_inputs.get(name) != Some(drvs) {
            changed.insert(name.clone());
        }
    }
    for name in base_inputs.keys() {
        if !cur_inputs.contains_key(name) {
            changed.insert(name.clone());
        }
    }
    changed
}

/// Map each input derivation's output *name* to the set of input `.drv` paths
/// contributing it, from a derivation's `inputDrvs` object.
fn inputs_by_name(drv: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let mut by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let Some(inputs) = drv.get("inputDrvs").and_then(Value::as_object) else {
        return by_name;
    };
    for path in inputs.keys() {
        if let Some(name) = output_name(path) {
            by_name.entry(name.to_owned()).or_default().insert(path.clone());
        }
    }
    by_name
}

/// The string array at `field`, or empty.
fn set_field<'a>(drv: &'a Value, field: &str) -> &'a [Value] {
    drv.get(field).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// The set of full store paths in a JSON string array. Full paths (not names):
/// a store path encodes its content hash, so comparing paths detects a content
/// change even when the name is unchanged.
fn path_set(paths: &[Value]) -> BTreeSet<&str> {
    paths.iter().filter_map(Value::as_str).collect()
}

/// The `<name>` of a `/nix/store/<hash>-<name>` path (no `.drv` stripping).
fn store_path_name(path: &str) -> Option<&str> {
    path.rsplit('/').next()?.get(33..).filter(|name| !name.is_empty())
}

/// Build-environment keys whose value differs between the two derivations.
fn changed_env_keys(cur: &Value, base: &Value) -> BTreeSet<String> {
    let empty = serde_json::Map::new();
    let cur_env = cur.get("env").and_then(Value::as_object).unwrap_or(&empty);
    let base_env = base.get("env").and_then(Value::as_object).unwrap_or(&empty);
    let mut changed = BTreeSet::new();
    for (key, value) in cur_env {
        if base_env.get(key) != Some(value) {
            changed.insert(key.clone());
        }
    }
    for key in base_env.keys() {
        if !cur_env.contains_key(key) {
            changed.insert(key.clone());
        }
    }
    changed
}

/// Render up to [`MAX_NAMED`] names, then "+N more", so a reason stays short.
fn summarise(names: &BTreeSet<String>) -> String {
    let shown: Vec<&str> = names.iter().take(MAX_NAMED).map(String::as_str).collect();
    let rest = names.len().saturating_sub(shown.len());
    if rest == 0 {
        shown.join(", ")
    } else {
        format!("{}, +{rest} more", shown.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_name_strips_hash_and_drv() {
        assert_eq!(
            output_name("/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bitflags-1.3.2.drv"),
            Some("bitflags-1.3.2")
        );
        assert_eq!(output_name("/nix/store/short.drv"), None);
        assert_eq!(output_name("not-a-store-path"), None);
    }

    #[test]
    fn source_content_change_is_detected_by_path() {
        // Same source name `src` but a different store hash (content changed): the
        // path sets must differ so "source changed" is reported, not collapsed.
        let cur = serde_json::json!({
            "inputSrcs": ["/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-src"]
        });
        let base = serde_json::json!({
            "inputSrcs": ["/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-src"]
        });
        assert_ne!(
            path_set(set_field(&cur, "inputSrcs")),
            path_set(set_field(&base, "inputSrcs"))
        );
    }

    #[test]
    fn changed_inputs_detects_a_bumped_dependency() {
        // Same logical input `rustc` but a different `.drv` hash -> changed.
        let cur = serde_json::json!({
            "inputDrvs": {
                "/nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-rustc-1.90.drv": {},
                "/nix/store/cccccccccccccccccccccccccccccccc-libc-2.40.drv": {}
            }
        });
        let base = serde_json::json!({
            "inputDrvs": {
                "/nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-rustc-1.89.drv": {},
                "/nix/store/cccccccccccccccccccccccccccccccc-libc-2.40.drv": {}
            }
        });
        assert_eq!(
            changed_input_names(&cur, &base),
            BTreeSet::from(["rustc-1.90".to_owned(), "rustc-1.89".to_owned()])
        );
    }

    #[test]
    fn store_path_name_distinguishes_dash_suffixed_names() {
        // The baseline lookup matches on this exact name, so `serde` must NOT be
        // considered the same output as `cargo-package-serde` (a real collision
        // shape in the store) -- otherwise the root cause is diffed against an
        // unrelated derivation.
        let h = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(
            store_path_name(&format!("/nix/store/{h}-serde-1.0.0")),
            Some("serde-1.0.0")
        );
        assert_eq!(
            store_path_name(&format!("/nix/store/{h}-cargo-package-serde-1.0.0")),
            Some("cargo-package-serde-1.0.0")
        );
    }

    #[test]
    fn summarise_caps_the_list() {
        let names = BTreeSet::from(["a", "b", "c", "d", "e"].map(ToOwned::to_owned));
        assert_eq!(summarise(&names), "a, b, c, +2 more");
    }
}
