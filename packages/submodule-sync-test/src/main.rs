//! Integration test for the `Bump submodules` step of
//! `.github/workflows/update-flake-lock.yml`.
//!
//! Ported from `.github/scripts/test-update-flake-lock-direct.sh`, which was
//! added without a `shell-allowlist.txt` entry and so failed the shell fence
//! (#3823) on every `nix run .#lint`. The allowlist only shrinks, so the fix
//! is this port rather than a new entry.
//!
//! The step under test is bash living inside a workflow YAML, so the test
//! still extracts that block and runs it under `bash`. What the fence targets
//! is committed shell and generated-shell call sites; executing the workflow's
//! own body is the thing being tested and cannot be ported away from here.
//!
//! Three behaviours are pinned:
//!
//! 1. A stale pin advances the gitlink and `flake.lock` to `index/main`
//!    together. Either one alone is a broken tree.
//! 2. A current pin is a true no-op, creating no commit.
//! 3. A push that loses a race against an unrelated `ix/main` commit retries
//!    onto the new tip and preserves the other commit rather than clobbering
//!    it.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use color_eyre::eyre::{Result, bail, eyre};

/// Run `git` with the real binary, failing loudly with the captured stderr.
///
/// Every fixture step goes through here so a broken fixture reports the git
/// error rather than surfacing later as a confusing assertion failure.
fn git(args: &[&str]) -> Result<String> {
    let out = Command::new("git").args(args).output()?;
    if !out.status.success() {
        bail!(
            "git {:?} failed ({}): {}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

fn git_init(path: &Path) -> Result<()> {
    let p = path.to_str().ok_or_else(|| eyre!("non-utf8 path"))?;
    git(&["init", "--quiet", "--initial-branch=main", p])?;
    git(&["-C", p, "config", "user.name", "test"])?;
    git(&["-C", p, "config", "user.email", "test@example.com"])?;
    Ok(())
}

fn write_exec(path: &Path, body: &str) -> Result<()> {
    fs::write(path, body)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// Pull the `Bump submodules` step's `run:` body out of the workflow.
///
/// Keyed on the step name, matching the shell original. A rename upstream
/// makes this fail with a clear message instead of silently testing nothing,
/// which is the failure mode that matters: a test that extracts an empty
/// script passes every assertion.
fn bump_submodules_script(workflow: &Path) -> Result<String> {
    let doc: serde_norway::Value = serde_norway::from_str(&fs::read_to_string(workflow)?)?;
    let steps = doc
        .get("jobs")
        .and_then(|j| j.get("update-flake-lock"))
        .and_then(|j| j.get("steps"))
        .and_then(|s| s.as_sequence())
        .ok_or_else(|| eyre!("no jobs.update-flake-lock.steps in {}", workflow.display()))?;

    steps
        .iter()
        .find(|s| s.get("name").and_then(|n| n.as_str()) == Some("Bump submodules"))
        .and_then(|s| s.get("run"))
        .and_then(|r| r.as_str())
        .map(str::to_owned)
        .ok_or_else(|| eyre!("no 'Bump submodules' step with a run: body"))
}

/// The lock shape ix uses for its path input: a `rev` the worker must rewrite.
fn seed_lock(rev: &str, timestamp: i64) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "nodes": {
            "root": {"inputs": {"index": "index"}},
            "index": {
                "locked": {
                    "lastModified": timestamp,
                    "path": "./index",
                    "rev": rev,
                    "type": "path"
                }
            }
        },
        "root": "root"
    }))
    .expect("static json")
}

struct Fixture {
    tmp: PathBuf,
    worker_sh: PathBuf,
    fake_bin: PathBuf,
    ix_git: PathBuf,
}

impl Fixture {
    /// Run the extracted workflow step against the worker checkout.
    ///
    /// `extra_env` carries the race-injection wiring; it is empty for the
    /// straightforward runs.
    fn run_worker(&self, extra_env: &[(&str, &str)]) -> Result<()> {
        let path = format!(
            "{}:{}",
            self.fake_bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut cmd = Command::new("bash");
        cmd.arg(&self.worker_sh)
            .current_dir(self.tmp.join("worker"))
            .env("DIRECT_PUSH", "true")
            .env("GH_TOKEN", "test")
            .env("GITHUB_REPOSITORY", "test/ix")
            .env("GIT_CONFIG_GLOBAL", self.tmp.join("gitconfig"))
            .env("SUBMODULE_PATHS", "index")
            .env("TRIGGER_USER", "")
            .env("UPDATE_REMOTE_URL", &self.ix_git)
            .env("PATH", path);
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let out = cmd.output()?;
        if !out.status.success() {
            bail!(
                "worker failed ({}):\nstdout:\n{}\nstderr:\n{}",
                out.status,
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(())
    }

    /// The gitlink and the lock's recorded rev, which must always agree.
    fn pinned(&self) -> Result<(String, String)> {
        let dir = self.ix_git.to_str().ok_or_else(|| eyre!("non-utf8"))?;
        let gitlink = git(&["--git-dir", dir, "rev-parse", "main:index"])?;
        let lock = git(&["--git-dir", dir, "show", "main:flake.lock"])?;
        let locked: serde_json::Value = serde_json::from_str(&lock)?;
        let rev = locked["nodes"]["index"]["locked"]["rev"]
            .as_str()
            .ok_or_else(|| eyre!("lock has no nodes.index.locked.rev"))?
            .to_owned();
        Ok((gitlink, rev))
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;

    let repo_root = PathBuf::from(
        std::env::args()
            .nth(1)
            .or_else(|| std::env::var("REPO_ROOT").ok())
            .unwrap_or_else(|| ".".to_owned()),
    );
    let workflow = repo_root.join(".github/workflows/update-flake-lock.yml");
    if !workflow.exists() {
        bail!(
            "no workflow at {} -- pass the repo root as argv[1] or set REPO_ROOT",
            workflow.display()
        );
    }

    let tempdir = tempfile::tempdir()?;
    let tmp = tempdir.path().to_path_buf();

    // Two source commits: the caller's parent starts at `old` while index/main
    // has already moved to `new`.
    let seed = tmp.join("index-seed");
    let index_git = tmp.join("index.git");
    git_init(&seed)?;
    let seed_s = seed.to_str().ok_or_else(|| eyre!("non-utf8"))?;
    fs::write(seed.join("version"), "old\n")?;
    git(&["-C", seed_s, "add", "version"])?;
    git(&["-C", seed_s, "commit", "--quiet", "-m", "old"])?;
    let old_source = git(&["-C", seed_s, "rev-parse", "HEAD"])?;
    git(&[
        "clone",
        "--quiet",
        "--bare",
        seed_s,
        index_git.to_str().ok_or_else(|| eyre!("non-utf8"))?,
    ])?;
    fs::write(seed.join("version"), "new\n")?;
    git(&["-C", seed_s, "commit", "--quiet", "-am", "new"])?;
    let new_source = git(&["-C", seed_s, "rev-parse", "HEAD"])?;
    let index_git_s = index_git.to_str().ok_or_else(|| eyre!("non-utf8"))?;
    git(&["-C", seed_s, "push", "--quiet", index_git_s, "main"])?;

    // Minimal caller repository with ix's path-input lock shape.
    let ix_seed = tmp.join("ix-seed");
    git_init(&ix_seed)?;
    let ix_seed_s = ix_seed.to_str().ok_or_else(|| eyre!("non-utf8"))?;
    git(&[
        "-c",
        "protocol.file.allow=always",
        "-C",
        ix_seed_s,
        "submodule",
        "add",
        "--quiet",
        index_git_s,
        "index",
    ])?;
    let sub = ix_seed.join("index");
    let sub_s = sub.to_str().ok_or_else(|| eyre!("non-utf8"))?;
    git(&["-C", sub_s, "checkout", "--quiet", &old_source])?;
    git(&[
        "-C",
        ix_seed_s,
        "config",
        "-f",
        ".gitmodules",
        "submodule.index.branch",
        "main",
    ])?;
    let old_timestamp: i64 = git(&["-C", sub_s, "show", "-s", "--format=%ct", &old_source])?.parse()?;
    fs::write(
        ix_seed.join("flake.lock"),
        seed_lock(&old_source, old_timestamp),
    )?;
    git(&[
        "-C",
        ix_seed_s,
        "add",
        ".gitmodules",
        "index",
        "flake.lock",
    ])?;
    git(&["-C", ix_seed_s, "commit", "--quiet", "-m", "initial"])?;

    let ix_git = tmp.join("ix.git");
    let ix_git_s = ix_git.to_str().ok_or_else(|| eyre!("non-utf8"))?;
    git(&["clone", "--quiet", "--bare", ix_seed_s, ix_git_s])?;
    git(&[
        "clone",
        "--quiet",
        ix_git_s,
        tmp.join("worker").to_str().ok_or_else(|| eyre!("non-utf8"))?,
    ])?;

    // The step re-stamps the lock itself, so a no-op `nix` proves that without
    // evaluating a real flake or touching the network.
    let fake_bin = tmp.join("fake-bin");
    fs::create_dir_all(&fake_bin)?;
    write_exec(&fake_bin.join("nix"), "#!/usr/bin/env bash\nexit 0\n")?;

    let worker_sh = tmp.join("worker.sh");
    fs::write(&worker_sh, bump_submodules_script(&workflow)?)?;
    let syntax = Command::new("bash").arg("-n").arg(&worker_sh).output()?;
    if !syntax.status.success() {
        bail!(
            "extracted step is not valid bash: {}",
            String::from_utf8_lossy(&syntax.stderr)
        );
    }

    git(&[
        "config",
        "--file",
        tmp.join("gitconfig").to_str().ok_or_else(|| eyre!("non-utf8"))?,
        "protocol.file.allow",
        "always",
    ])?;

    let fx = Fixture {
        tmp: tmp.clone(),
        worker_sh,
        fake_bin: fake_bin.clone(),
        ix_git: ix_git.clone(),
    };

    // 1. A stale pin advances both the gitlink and the lock.
    fx.run_worker(&[])?;
    let (gitlink, locked) = fx.pinned()?;
    if gitlink != new_source || locked != new_source {
        bail!(
            "direct update did not move both the gitlink and lock to index/main\n\
             gitlink={gitlink}\nlocked={locked}\nexpected={new_source}"
        );
    }

    // 2. A current pin is a true no-op.
    let before = git(&["--git-dir", ix_git_s, "rev-parse", "main"])?;
    fx.run_worker(&[])?;
    let after = git(&["--git-dir", ix_git_s, "rev-parse", "main"])?;
    if before != after {
        bail!("current-pin run created an unexpected commit ({before} -> {after})");
    }

    // 3. Advance index again, then land an unrelated ix/main commit in between
    //    the worker's fetch and its first push. The first push must lose, and
    //    the retry must rebuild on the new tip without dropping the other
    //    commit.
    fs::write(seed.join("version"), "newer\n")?;
    git(&["-C", seed_s, "commit", "--quiet", "-am", "newer"])?;
    let newest_source = git(&["-C", seed_s, "rev-parse", "HEAD"])?;
    git(&["-C", seed_s, "push", "--quiet", index_git_s, "main"])?;

    let racer = tmp.join("racer");
    let racer_s = racer.to_str().ok_or_else(|| eyre!("non-utf8"))?;
    git(&["clone", "--quiet", ix_git_s, racer_s])?;
    git(&["-C", racer_s, "config", "user.name", "racer"])?;
    git(&["-C", racer_s, "config", "user.email", "racer@example.com"])?;
    fs::write(racer.join("race.txt"), "preserve me\n")?;
    git(&["-C", racer_s, "add", "race.txt"])?;
    git(&["-C", racer_s, "commit", "--quiet", "-m", "race"])?;

    // A `git` shim on PATH ahead of the real one. It fires exactly once, on
    // the worker's first push to main, so the race is deterministic rather
    // than timing-dependent.
    let real_git = String::from_utf8(
        Command::new("sh")
            .arg("-c")
            .arg("command -v git")
            .output()?
            .stdout,
    )?
    .trim()
    .to_owned();
    write_exec(
        &fake_bin.join("git"),
        "#!/usr/bin/env bash\n\
         set -euo pipefail\n\
         if [ \"${1:-}\" = push ] && [ \"${2:-}\" = origin ] &&\n\
         \x20  [ \"${3:-}\" = HEAD:refs/heads/main ] && [ ! -e \"$RACE_MARKER\" ]; then\n\
         \x20 : >\"$RACE_MARKER\"\n\
         \x20 \"$REAL_GIT\" -C \"$RACE_WORK\" push --quiet origin main\n\
         fi\n\
         exec \"$REAL_GIT\" \"$@\"\n",
    )?;

    fx.run_worker(&[
        (
            "RACE_MARKER",
            tmp.join("race-fired").to_str().ok_or_else(|| eyre!("non-utf8"))?,
        ),
        ("RACE_WORK", racer_s),
        ("REAL_GIT", &real_git),
    ])?;

    if !tmp.join("race-fired").exists() {
        bail!("race was never injected -- the shim's push match is stale, so this run proved nothing");
    }

    let (gitlink, locked) = fx.pinned()?;
    let race_file = git(&["--git-dir", ix_git_s, "show", "main:race.txt"])?;
    if gitlink != newest_source || locked != newest_source {
        bail!(
            "race retry did not converge to the newest index/main\n\
             gitlink={gitlink}\nlocked={locked}\nexpected={newest_source}"
        );
    }
    if race_file.trim() != "preserve me" {
        bail!("race retry lost the concurrent ix/main commit (race.txt={race_file:?})");
    }

    println!("submodule-sync-test: PASS");
    Ok(())
}
