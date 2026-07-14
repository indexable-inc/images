//! Regenerate a de-forked package's `patches/` series when its upstream base
//! moves, by round-tripping through a real `git rebase`. The patch folder is a
//! serialization of a git branch: fetch the old base (from the committed
//! flake.lock) and the new base (from the working-tree flake.lock), replay the
//! series onto the old base with `git am`, `git rebase --onto <new> <old>`
//! (git's 3-way merge absorbs shifted line numbers and drifted context;
//! mergiraf resolves structural conflicts), then re-export with
//! `git format-patch` so the files come back with fresh context, deterministic
//! bytes, and authorship/messages preserved.
//!
//! No fallbacks: an unresolved conflict stops loudly, printing the scratch repo
//! path and a resume command. The resume path owns `git rebase --continue`,
//! rerere export, patch serialization, and DAG regeneration, so a manual
//! resolution cannot get stranded in the temporary repo.
//!
//! Committed rerere: a resolution cache per fork package
//! (packages/<name>/rerere/, git rr-cache format) is seeded into the scratch
//! repo before rebasing and exported back after a manual resolution, so a
//! conflict resolved once replays on later runs. Every replayed resolution is
//! printed loudly so nothing lands silently; the re-exported patches remain the
//! resolution of record and the flake checks are the correctness gate.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dag;
use crate::fork::{self, Fork};
use crate::git;

const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// The rebase loop resumes through rerere-staged resolutions; past this many
/// continuations without an unresolved path something is wedged, so stop loud.
const MAX_CONTINUES: u32 = 100;

/// Set up a scratch git repo for `fork`: mergiraf merge driver, zdiff3
/// conflict style, and committed rerere seeded from `<patchDir>/rerere/`.
/// One owner for scratch setup so the rebase and DAG regen paths build
/// identical trees. The dir is deliberately NOT auto-cleaned: on an unresolved
/// conflict it is the human's resume workspace.
fn scratch_init(fork: &Fork) -> Result<PathBuf> {
    let scratch = tempfile::Builder::new()
        .prefix(&format!("rebase-patches-{}.", fork.name))
        .tempdir()?
        .keep();

    git::run(&scratch, &["init", "--quiet"])?;
    // mergiraf registered as the merge driver for the languages it supports
    // (git reads merge.conflictStyle + the `merge` gitattribute); zdiff3 gives
    // readable textual conflict markers when mergiraf declines. The
    // `* merge=mergiraf` mapping goes in `.git/info/attributes`, NOT a
    // worktree `.gitattributes`: an untracked worktree file would collide with
    // a tracked `.gitattributes` in the fetched upstream tree on checkout.
    git::run(&scratch, &["config", "merge.conflictStyle", "zdiff3"])?;
    // rerere replays a resolution the moment the same conflict recurs. The
    // committed cache (seeded below) makes it earn its keep for conflicts that
    // recur across different DAG branches or repeated upstream churn: the same
    // textual clash resolved once is replayed on every later rebase.
    git::run(&scratch, &["config", "rerere.enabled", "true"])?;
    git::run(&scratch, &["config", "merge.mergiraf.name", "mergiraf syntax-aware merge"])?;
    git::run(
        &scratch,
        &[
            "config",
            "merge.mergiraf.driver",
            "mergiraf merge --git %O %A %B -s %S -x %X -y %Y -p %P -l %L",
        ],
    )?;
    let info = scratch.join(".git/info");
    fs::create_dir_all(&info)?;
    fs::write(info.join("attributes"), "* merge=mergiraf\n")?;

    // Seed the committed rerere cache so prior resolutions replay.
    let rr_committed = fork.patch_dir_abs()?.join("rerere");
    if rr_committed.exists() {
        dag::copy_tree(&rr_committed, &scratch.join(".git/rr-cache"))?;
        println!(
            "{CYAN}rebase-patches: {}: seeded committed rerere cache from {}/rerere{RESET}",
            fork.name, fork.patch_dir
        );
    }
    Ok(scratch)
}

/// Export rerere entries created or touched during the rebase back to the
/// committed cache. Only entries with a recorded resolution (a `postimage`)
/// are exported; a bare `preimage` with no resolution is an unresolved
/// conflict we must not persist as if it were a fix. Loud so a new committed
/// resolution is never silent.
fn rerere_export(fork: &Fork, scratch: &Path) -> Result<()> {
    let rr_scratch = scratch.join(".git/rr-cache");
    if !rr_scratch.exists() {
        return Ok(());
    }
    let mut resolved: Vec<String> = Vec::new();
    for entry in fs::read_dir(&rr_scratch)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join("postimage").exists() {
            resolved.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    if resolved.is_empty() {
        return Ok(());
    }
    resolved.sort();

    let rr_committed = fork.patch_dir_abs()?.join("rerere");
    fs::create_dir_all(&rr_committed)?;
    for key in &resolved {
        // Replace, never merge: a stale committed entry must not shadow the
        // fresh one.
        let dest = rr_committed.join(key);
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
        }
        dag::copy_tree(&rr_scratch.join(key), &dest)?;
        // `thisimage` is Git's transient current-conflict snapshot. Replaying
        // a resolution needs only the numbered or unnumbered pre/postimage
        // pairs.
        for entry in fs::read_dir(&dest)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with("thisimage") {
                fs::remove_file(entry.path())?;
            }
        }
    }
    println!(
        "{YELLOW}rebase-patches: {}: exported {} rerere resolution(s) to {}/rerere: {}{RESET}",
        fork.name,
        resolved.len(),
        fork.patch_dir,
        resolved.join(", ")
    );
    Ok(())
}

/// Print every resolution rerere REPLAYED during the rebase. Reads git's own
/// trace: rerere logs "Resolved '<file>' using previous resolution." on
/// replay. We surface the replays so nothing lands silently.
fn rerere_report_replays(fork: &Fork, log: &str) {
    let replays: Vec<&str> = log
        .lines()
        .filter(|l| l.contains("using previous resolution"))
        .collect();
    if replays.is_empty() {
        return;
    }
    println!(
        "{YELLOW}rebase-patches: {}: rerere REPLAYED {} previously-recorded resolution(s):{RESET}",
        fork.name,
        replays.len()
    );
    for replay in &replays {
        println!("  {}", replay.trim());
    }
    println!(
        "{YELLOW}rebase-patches: {}: review these replayed hunks; the re-exported patches are the resolution of record.{RESET}",
        fork.name
    );
}

/// Regenerate dag.json next to a fork package's patches. Derives the sparse
/// dependency DAG by apply-tests against `base` in a fresh scratch repo, then
/// writes the deterministic bytes. Shared by the post-rebase publish and the
/// standalone `dag` subcommand.
fn regen_dag(fork: &Fork, base: &str) -> Result<()> {
    let patch_dir = fork.patch_dir_abs()?;
    let patches = dag::patches_in(&patch_dir)?;
    if patches.is_empty() {
        bail!(
            "rebase-patches: {}: no *.patch files in {}",
            fork.name,
            patch_dir.display()
        );
    }

    let scratch = tempfile::Builder::new()
        .prefix(&format!("rebase-patches-dag-{}.", fork.name))
        .tempdir()?;
    git::run(scratch.path(), &["init", "--quiet"])?;
    // DAG derivation must be config-independent (a developer's global
    // rerere.enabled=true would silently corrupt the apply-tests), and the
    // apply-tests commit, so the scratch needs an identity even where the
    // developer has none configured.
    dag::neutralize_config(scratch.path())?;
    dag::identity(scratch.path())?;
    git::run(
        scratch.path(),
        &["fetch", "--quiet", "--filter=blob:none", &fork.url, base],
    )?;
    git::run(scratch.path(), &["checkout", "--quiet", "--detach", base])?;

    let nodes = dag::derive(scratch.path(), base, &patches)?;
    let edges: usize = nodes.iter().map(|n| n.deps.len()).sum();
    let roots = nodes.iter().filter(|n| n.deps.is_empty()).count();
    let doc = dag::document(base, nodes);
    fs::write(patch_dir.join("dag.json"), dag::to_json(&doc)?)?;
    println!(
        "{GREEN}rebase-patches: {}: regenerated dag.json {} nodes, {edges} edges, {roots} roots{RESET}",
        fork.name,
        patches.len()
    );

    // Surface the inline-reason requirement at authoring time: the
    // `patch-dag-<name>` flake check fails any patch whose commit message
    // states no reason, and regenerating the DAG is the first tooling a new
    // patch meets. A warning, not an error: this run's job is the rebase/DAG,
    // and the flake check stays the gate, so a base bump is never blocked on a
    // pre-existing message.
    let mut mute: Vec<&str> = Vec::new();
    for patch in &patches {
        if !dag::body_has_reason(&patch.file)? {
            mute.push(&patch.name);
        }
    }
    if !mute.is_empty() {
        println!(
            "{YELLOW}rebase-patches: {}: {} patch(es) state no reason in their commit-message body and will fail the patch-dag-{} check; write the why into: {}{RESET}",
            fork.name,
            mute.len(),
            fork.name,
            mute.join(", ")
        );
    }
    Ok(())
}

/// Resume state binding a scratch repo to the fork and target revision,
/// preventing an arbitrary checkout from being serialized into a package's
/// patch directory.
#[derive(Serialize, Deserialize)]
struct State {
    fork: String,
    old: String,
    new: String,
}

fn state_file(scratch: &Path) -> PathBuf {
    scratch.join(".git/rebase-patches-state.json")
}

/// Bail with the resume instructions when the scratch repo holds unresolved
/// conflicts, exporting any resolutions recorded so far first.
fn bail_if_unresolved(fork: &Fork, scratch: &Path, new: &str) -> Result<()> {
    let conflicts = git::stdout(scratch, &["diff", "--name-only", "--diff-filter=U"])?;
    if conflicts.is_empty() {
        return Ok(());
    }
    rerere_export(fork, scratch)?;
    bail!(
        "rebase-patches: {}: rebase onto {new} has unresolved conflicts in [{}]; resolve them in {}, `git -C {} add <files>`, then `nix run .#rebase-patches -- resume {} {}`",
        fork.name,
        conflicts.lines().collect::<Vec<_>>().join(", "),
        scratch.display(),
        scratch.display(),
        fork.name,
        scratch.display()
    );
}

/// Serialize the finished scratch branch back into the patch dir and
/// regenerate its DAG; removes the scratch repo.
fn publish(fork: &Fork, scratch: &Path, new: &str) -> Result<()> {
    let based = git::output(scratch, &["merge-base", "--is-ancestor", new, "HEAD"])?;
    if !based.status.success() {
        bail!(
            "rebase-patches: {}: scratch HEAD is not based on expected upstream rev {new}: {}",
            fork.name,
            scratch.display()
        );
    }

    let patch_dir = fork.patch_dir_abs()?;
    for patch in dag::patches_in(&patch_dir)? {
        fs::remove_file(&patch.file)?;
    }
    let out = git::utf8(&patch_dir)?.to_owned();
    let range = format!("{new}..HEAD");
    git::run(
        scratch,
        &["format-patch", "--zero-commit", "--no-signature", "--no-stat", "-N", "-o", &out, &range],
    )?;
    println!(
        "{GREEN}rebase-patches: {}: regenerated {} patches in {}{RESET}",
        fork.name,
        dag::patches_in(&patch_dir)?.len(),
        fork.patch_dir
    );

    fs::remove_dir_all(scratch)
        .with_context(|| format!("remove scratch repo {}", scratch.display()))?;
    regen_dag(fork, new)
}

/// Drive `git rebase --continue` until the rebase finishes or stops on an
/// unresolved path; returns the accumulated rebase log for replay reporting.
fn continue_until_done(
    fork: &Fork,
    scratch: &Path,
    new: &str,
    mut status: std::process::ExitStatus,
    mut log: String,
) -> Result<String> {
    let mut stops: u32 = 0;
    while !status.success() {
        let conflicts = git::stdout(scratch, &["diff", "--name-only", "--diff-filter=U"])?;
        stops += 1;
        if !conflicts.is_empty() || stops > MAX_CONTINUES {
            rerere_report_replays(fork, &log);
            bail_if_unresolved(fork, scratch, new)?;
            bail!(
                "rebase-patches: {}: rebase onto {new} stopped without an unresolved path after {stops} continuation attempts: {log}",
                fork.name
            );
        }
        // rerere replayed and staged a recorded resolution for every
        // conflicted path; git still stops so a human could review, but the
        // committed cache is the reviewed resolution of record, so resume the
        // rebase.
        let resumed = git::output(
            scratch,
            &["-c", "core.editor=true", "rebase", "--continue"],
        )?;
        status = resumed.status;
        log.push_str(&String::from_utf8_lossy(&resumed.stdout));
        log.push_str(&String::from_utf8_lossy(&resumed.stderr));
    }
    Ok(log)
}

/// Regenerate one fork package's patch series: `old` -> `new` base revs.
fn rebase_one(fork: &Fork, old: &str, new: &str) -> Result<()> {
    println!("{CYAN}rebase-patches: {}: {old} -> {new}{RESET}", fork.name);

    let patch_dir = fork.patch_dir_abs()?;
    let patches = dag::patches_in(&patch_dir)?;
    if patches.is_empty() {
        bail!(
            "rebase-patches: {}: no *.patch files in {}",
            fork.name,
            patch_dir.display()
        );
    }

    let scratch = scratch_init(fork)?;
    let state = State { fork: fork.name.clone(), old: old.to_owned(), new: new.to_owned() };
    fs::write(state_file(&scratch), format!("{}\n", serde_json::to_string_pretty(&state)?))?;
    // Blobless fetch of just the two revs we round-trip between.
    git::run(
        &scratch,
        &["fetch", "--quiet", "--filter=blob:none", &fork.url, old, new],
    )?;
    git::run(&scratch, &["checkout", "--quiet", "--detach", old])?;

    // Replay the committed series onto the old base: our branch, bit-identical.
    let mut am = vec!["am".to_owned()];
    am.extend(patches.iter().map(|p| p.file.display().to_string()));
    let am_args: Vec<&str> = am.iter().map(String::as_str).collect();
    let replayed = git::output(&scratch, &am_args)?;
    if !replayed.status.success() {
        // Best-effort abort: `git am` may have failed before a session started,
        // in which case the abort itself fails and must not mask the real error.
        git::output(&scratch, &["am", "--abort"])?;
        bail!(
            "rebase-patches: {}: `git am` failed replaying the committed series onto the pinned base {old}; scratch repo: {}",
            fork.name,
            scratch.display()
        );
    }

    // Rebase our branch onto the new base. 3-way + mergiraf absorb the
    // mechanical drift; a real semantic collision stops here. Capture the
    // combined output so we can surface any rerere replays.
    let rebased = git::output(&scratch, &["rebase", "--onto", new, old])?;
    let mut log = String::from_utf8_lossy(&rebased.stdout).into_owned();
    log.push_str(&String::from_utf8_lossy(&rebased.stderr));
    let log = continue_until_done(fork, &scratch, new, rebased.status, log)?;

    // Loudly report and persist any rerere resolutions that fired.
    rerere_report_replays(fork, &log);
    rerere_export(fork, &scratch)?;
    publish(fork, &scratch, new)
}

/// Continue a stopped rebase in place, then publish the same artifacts as a
/// conflict-free run. The state file binds the temporary repo to the fork and
/// target revision.
pub fn resume(name: &str, scratch: &Path, mapping: Option<&Path>) -> Result<()> {
    let fork = fork::select(Some(name), mapping)?.remove(0);
    let state_path = state_file(scratch);
    if !state_path.exists() {
        bail!(
            "rebase-patches: resume state missing from scratch repo: {}",
            state_path.display()
        );
    }
    let state: State = serde_json::from_str(&fs::read_to_string(&state_path)?)
        .with_context(|| format!("parse {}", state_path.display()))?;
    let new = fork::locked_rev(&working_lock()?, &fork.input)?;
    if state.fork != fork.name || state.new != new {
        bail!(
            "rebase-patches: resume state does not match {} at {new}: {}",
            fork.name,
            serde_json::to_string(&state)?
        );
    }

    bail_if_unresolved(&fork, scratch, &new)?;
    if scratch.join(".git/rebase-merge").exists() || scratch.join(".git/rebase-apply").exists() {
        let resumed = git::output(scratch, &["-c", "core.editor=true", "rebase", "--continue"])?;
        let mut log = String::from_utf8_lossy(&resumed.stdout).into_owned();
        log.push_str(&String::from_utf8_lossy(&resumed.stderr));
        log = continue_until_done(&fork, scratch, &new, resumed.status, log)?;
        rerere_report_replays(&fork, &log);
    }

    rerere_export(&fork, scratch)?;
    publish(&fork, scratch, &new)
}

/// `dag` subcommand: regenerate dag.json for one or all fork packages against
/// the currently-pinned base (working-tree flake.lock), without a rebase.
pub fn dag_all(name: Option<&str>, mapping: Option<&Path>) -> Result<()> {
    let forks = fork::select(name, mapping)?;
    let lock = working_lock()?;
    for fork in &forks {
        let base = fork::locked_rev(&lock, &fork.input)?;
        regen_dag(fork, &base)?;
    }
    Ok(())
}

/// Default run: rebase every selected fork whose base moved between the
/// committed flake.lock (HEAD) and the working tree.
pub fn run(name: Option<&str>, mapping: Option<&Path>) -> Result<()> {
    let selected = fork::select(name, mapping)?;

    let cwd = std::env::current_dir()?;
    let old_lock: Value = serde_json::from_str(&git::stdout(&cwd, &["show", "HEAD:flake.lock"])?)
        .context("parse HEAD:flake.lock")?;
    let new_lock = working_lock()?;

    let mut did_any = false;
    for fork in &selected {
        let old_rev = fork::locked_rev(&old_lock, &fork.input)?;
        let new_rev = fork::locked_rev(&new_lock, &fork.input)?;
        if old_rev == new_rev {
            println!(
                "rebase-patches: {}: base unchanged ({old_rev}); nothing to do",
                fork.name
            );
            continue;
        }
        rebase_one(fork, &old_rev, &new_rev)?;
        did_any = true;
    }
    if !did_any {
        println!("rebase-patches: no fork input moved; patches are up to date");
    }
    Ok(())
}

/// The working-tree flake.lock, resolved against the invocation cwd (the repo
/// root by contract).
fn working_lock() -> Result<Value> {
    serde_json::from_str(&fs::read_to_string("flake.lock").context("read flake.lock (run from the repo root)")?)
        .context("parse flake.lock")
}
