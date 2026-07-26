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
//! Three behaviours are pinned, one function each at the bottom of this file:
//!
//! 1. Stale pins advance every gitlink and every `flake.lock` node to the
//!    matching remote main together. Any partial move is a broken tree.
//! 2. A current pin is a true no-op, creating no commit.
//! 3. A push that loses a race against an unrelated `ix/main` commit retries
//!    onto the new tip and preserves the other commit rather than clobbering
//!    it, without disturbing the sources it had no reason to move.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use color_eyre::eyre::{Result, bail, eyre};

/// The sources the caller pins, in the order it mounts them.
///
/// `nox` is here because the workflow now syncs a second, private source: a
/// run that advanced only one of the two would still look like a pass against
/// a single-submodule fixture.
const SUBMODULES: [&str; 2] = ["index", "nox"];

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

/// A path as a `str`, for the many git invocations that take one.
///
/// Every path here is derived from a `tempfile` root, so a non-utf8 path means
/// a broken environment rather than unusual input: worth a clear error, not
/// worth threading `OsStr` through the fixture.
fn utf8(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| eyre!("non-utf8 path: {}", path.display()))
}

fn git_init(path: &Path) -> Result<()> {
    let p = utf8(path)?;
    git(&["init", "--quiet", "--initial-branch=main", p])?;
    git(&["-C", p, "config", "user.name", "test"])?;
    git(&["-C", p, "config", "user.email", "test@example.com"])?;
    // Fixture commits must not inherit the caller's signing config. A machine
    // with `commit.gpgsign` on (ssh format is common) fails fixture setup with
    // `cannot run ssh-keygen: No such file or directory` / `failed to write
    // commit object`, which reads as a broken test rather than a broken
    // environment. The shell original had the same hole: it pointed
    // GIT_CONFIG_GLOBAL at a throwaway file for the worker only, leaving these
    // setup commits on the developer's real config.
    git(&["-C", p, "config", "commit.gpgsign", "false"])?;
    git(&["-C", p, "config", "tag.gpgsign", "false"])?;
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

/// One gitlink-pinned source the caller tracks.
struct Submodule {
    /// Directory the caller mounts it at, and the key of its `flake.lock` node.
    name: &'static str,
    /// Working clone; every commit on this source is authored here.
    seed: PathBuf,
    /// Bare repository the caller's submodule tracks.
    bare: PathBuf,
    /// The revision the caller's initial commit pins: one behind `tip`, which
    /// is what gives the step something to move.
    stale: String,
    /// Current tip of the source's `main`, which a correct run converges onto.
    tip: String,
}

impl Submodule {
    /// Publish two commits: the stale one the caller will pin, and the tip the
    /// step must advance to.
    fn publish(tmp: &Path, name: &'static str) -> Result<Self> {
        let seed = tmp.join(format!("{name}-seed"));
        let bare = tmp.join(format!("{name}.git"));
        git_init(&seed)?;
        let stale = {
            let s = utf8(&seed)?;
            fs::write(seed.join("version"), "old\n")?;
            git(&["-C", s, "add", "version"])?;
            git(&["-C", s, "commit", "--quiet", "-m", "old"])?;
            let stale = git(&["-C", s, "rev-parse", "HEAD"])?;
            git(&["clone", "--quiet", "--bare", s, utf8(&bare)?])?;
            stale
        };
        let mut sub = Self {
            name,
            seed,
            bare,
            stale,
            tip: String::new(),
        };
        sub.advance("new")?;
        Ok(sub)
    }

    /// Author one more commit on the source and publish it, so `tip` is always
    /// what a correct run converges onto.
    fn advance(&mut self, version: &str) -> Result<()> {
        let tip = {
            let seed = utf8(&self.seed)?;
            fs::write(self.seed.join("version"), format!("{version}\n"))?;
            git(&["-C", seed, "commit", "--quiet", "-am", version])?;
            let tip = git(&["-C", seed, "rev-parse", "HEAD"])?;
            git(&["-C", seed, "push", "--quiet", utf8(&self.bare)?, "main"])?;
            tip
        };
        self.tip = tip;
        Ok(())
    }

    /// Mount the source in `caller` at its stale revision, tracking `main`.
    fn mount(&self, caller: &Path) -> Result<()> {
        let caller_s = utf8(caller)?;
        git(&[
            "-c",
            "protocol.file.allow=always",
            "-C",
            caller_s,
            "submodule",
            "add",
            "--quiet",
            utf8(&self.bare)?,
            self.name,
        ])?;
        let checkout = caller.join(self.name);
        git(&["-C", utf8(&checkout)?, "checkout", "--quiet", &self.stale])?;
        git(&[
            "-C",
            caller_s,
            "config",
            "-f",
            ".gitmodules",
            &format!("submodule.{}.branch", self.name),
            "main",
        ])?;
        Ok(())
    }

    /// This source's `flake.lock` node, pinned at the stale revision.
    fn stale_lock_node(&self) -> Result<serde_json::Value> {
        let timestamp: i64 = git(&[
            "-C",
            utf8(&self.seed)?,
            "show",
            "-s",
            "--format=%ct",
            &self.stale,
        ])?
        .parse()?;
        Ok(serde_json::json!({
            "locked": {
                "lastModified": timestamp,
                "path": format!("./{}", self.name),
                "rev": self.stale,
                "type": "path"
            }
        }))
    }
}

/// The lock shape ix uses for its path inputs: one `rev` per source, which the
/// worker must rewrite together with the matching gitlink.
fn seed_lock(subs: &[Submodule]) -> Result<String> {
    let mut inputs = serde_json::Map::new();
    let mut nodes = serde_json::Map::new();
    for sub in subs {
        inputs.insert(sub.name.to_owned(), serde_json::json!(sub.name));
        nodes.insert(sub.name.to_owned(), sub.stale_lock_node()?);
    }
    nodes.insert("root".to_owned(), serde_json::json!({"inputs": inputs}));
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "nodes": nodes,
        "root": "root"
    }))?)
}

/// Where `ix/main` says one source sits, read from both places that record it.
/// The whole point of the step is that these two never disagree.
struct Pinned {
    /// `main:<name>`, the gitlink recorded in the tree.
    gitlink: String,
    /// `nodes.<name>.locked.rev` in `main:flake.lock`.
    lock: String,
}

/// The caller repository, the sources it pins, and the extracted step.
///
/// Every path is derived from `tmp`, which the caller owns and deletes.
struct Fixture {
    tmp: PathBuf,
    /// Bare repository standing in for `ix`, whose gitlinks and lock must move.
    ix_git: PathBuf,
    /// Prepended to the worker's PATH: a no-op `nix`, and for the race case a
    /// `git` shim in front of the real one.
    fake_bin: PathBuf,
    /// The `Bump submodules` body, extracted from the workflow.
    worker_sh: PathBuf,
    /// The sources the caller pins, in mount order.
    subs: Vec<Submodule>,
}

impl Fixture {
    /// Lay out the repositories and extract the step under test.
    fn new(tmp: PathBuf, workflow: &Path) -> Result<Self> {
        let subs = SUBMODULES
            .into_iter()
            .map(|name| Submodule::publish(&tmp, name))
            .collect::<Result<Vec<_>>>()?;
        let fx = Self {
            ix_git: tmp.join("ix.git"),
            fake_bin: tmp.join("fake-bin"),
            worker_sh: tmp.join("worker.sh"),
            subs,
            tmp,
        };
        fx.seed_caller_repo()?;
        fx.stage_worker(workflow)?;
        Ok(fx)
    }

    /// Build the caller: every source mounted at its stale revision, plus ix's
    /// path-input lock recording those same revisions. Publishes it bare and
    /// clones the worker checkout the step runs in.
    fn seed_caller_repo(&self) -> Result<()> {
        let ix_seed = self.tmp.join("ix-seed");
        git_init(&ix_seed)?;
        let ix_seed_s = utf8(&ix_seed)?;
        for sub in &self.subs {
            sub.mount(&ix_seed)?;
        }
        fs::write(ix_seed.join("flake.lock"), seed_lock(&self.subs)?)?;
        let mut add = vec!["-C", ix_seed_s, "add", ".gitmodules", "flake.lock"];
        add.extend(self.subs.iter().map(|sub| sub.name));
        git(&add)?;
        git(&["-C", ix_seed_s, "commit", "--quiet", "-m", "initial"])?;

        git(&["clone", "--quiet", "--bare", ix_seed_s, utf8(&self.ix_git)?])?;
        git(&[
            "clone",
            "--quiet",
            utf8(&self.ix_git)?,
            utf8(&self.tmp.join("worker"))?,
        ])?;
        Ok(())
    }

    /// Extract the step, check it still parses as bash, and stand up the
    /// ambient state it runs against.
    ///
    /// The step re-stamps the lock itself, so a no-op `nix` proves that without
    /// evaluating a real flake or touching the network. The throwaway global
    /// gitconfig keeps `protocol.file.allow` off the developer's real config.
    fn stage_worker(&self, workflow: &Path) -> Result<()> {
        fs::create_dir_all(&self.fake_bin)?;
        write_exec(&self.fake_bin.join("nix"), "#!/usr/bin/env bash\nexit 0\n")?;

        fs::write(&self.worker_sh, bump_submodules_script(workflow)?)?;
        let syntax = Command::new("bash")
            .arg("-n")
            .arg(&self.worker_sh)
            .output()?;
        if !syntax.status.success() {
            bail!(
                "extracted step is not valid bash: {}",
                String::from_utf8_lossy(&syntax.stderr)
            );
        }

        git(&[
            "config",
            "--file",
            utf8(&self.tmp.join("gitconfig"))?,
            "protocol.file.allow",
            "always",
        ])?;
        Ok(())
    }

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
        let submodule_paths = self
            .subs
            .iter()
            .map(|sub| sub.name)
            .collect::<Vec<_>>()
            .join(" ");
        let mut cmd = Command::new("bash");
        cmd.arg(&self.worker_sh)
            .current_dir(self.tmp.join("worker"))
            .env("DIRECT_PUSH", "true")
            .env("GH_TOKEN", "test")
            .env("GITHUB_REPOSITORY", "test/ix")
            .env("GIT_CONFIG_GLOBAL", self.tmp.join("gitconfig"))
            .env("SUBMODULE_PATHS", submodule_paths)
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

    /// Resolve a revision in the `ix` bare repository.
    fn ix_rev(&self, spec: &str) -> Result<String> {
        git(&["--git-dir", utf8(&self.ix_git)?, "rev-parse", spec])
    }

    /// Read a file from `ix/main`.
    fn ix_show(&self, path: &str) -> Result<String> {
        git(&[
            "--git-dir",
            utf8(&self.ix_git)?,
            "show",
            &format!("main:{path}"),
        ])
    }

    /// One source's gitlink and lock rev, which must always agree.
    fn pinned(&self, name: &str) -> Result<Pinned> {
        let gitlink = self.ix_rev(&format!("main:{name}"))?;
        let locked: serde_json::Value = serde_json::from_str(&self.ix_show("flake.lock")?)?;
        let lock = locked["nodes"][name]["locked"]["rev"]
            .as_str()
            .ok_or_else(|| eyre!("lock has no nodes.{name}.locked.rev"))?
            .to_owned();
        Ok(Pinned { gitlink, lock })
    }

    /// Fail with `complaint` unless every source's gitlink and lock both sit
    /// on that source's current tip.
    ///
    /// Half-moved is the failure worth naming: a tree that builds the old
    /// source against the new lock. So is a source that moved when nothing
    /// asked it to, which is why this checks all of them every time.
    fn assert_pinned_at_tips(&self, complaint: &str) -> Result<()> {
        for sub in &self.subs {
            let Pinned { gitlink, lock } = self.pinned(sub.name)?;
            if gitlink != sub.tip || lock != sub.tip {
                bail!(
                    "{complaint} ({})\ngitlink={gitlink}\nlocked={lock}\nexpected={}",
                    sub.name,
                    sub.tip
                );
            }
        }
        Ok(())
    }

    /// Move one source's `main` forward, leaving the others where they are.
    fn advance_source(&mut self, name: &str, version: &str) -> Result<()> {
        self.subs
            .iter_mut()
            .find(|sub| sub.name == name)
            .ok_or_else(|| eyre!("no submodule named {name} in the fixture"))?
            .advance(version)
    }

    /// Clone `ix` and commit `race.txt` there, ready to be pushed from inside
    /// the worker's first push.
    fn stage_racer_commit(&self) -> Result<PathBuf> {
        let racer = self.tmp.join("racer");
        let racer_s = utf8(&racer)?;
        git(&["clone", "--quiet", utf8(&self.ix_git)?, racer_s])?;
        git(&["-C", racer_s, "config", "user.name", "racer"])?;
        git(&["-C", racer_s, "config", "user.email", "racer@example.com"])?;
        // Cloned, not `git_init`ed, so it needs the same signing opt-out; without
        // it this repo alone still picks up the caller's `commit.gpgsign`.
        git(&["-C", racer_s, "config", "commit.gpgsign", "false"])?;
        fs::write(racer.join("race.txt"), "preserve me\n")?;
        git(&["-C", racer_s, "add", "race.txt"])?;
        git(&["-C", racer_s, "commit", "--quiet", "-m", "race"])?;
        Ok(racer)
    }

    /// Put a `git` shim on the fixture's PATH ahead of the real one, and
    /// return the real one for the shim to exec.
    ///
    /// It fires exactly once, on the worker's first push to main, so the race
    /// is deterministic rather than timing-dependent.
    fn install_racing_git_shim(&self) -> Result<String> {
        let real_git = String::from_utf8(
            Command::new("sh")
                .arg("-c")
                .arg("command -v git")
                .output()?
                .stdout,
        )?
        .trim()
        .to_owned();
        // Positional parameters are spelled `$1` behind the arity test rather
        // than `${1:-}`, so the script contains no `{...}`: clippy's
        // `literal_string_with_formatting_args` reads a braced parameter
        // expansion inside a Rust string literal as a stray format argument.
        write_exec(
            &self.fake_bin.join("git"),
            "#!/usr/bin/env bash\n\
             set -euo pipefail\n\
             if [ \"$#\" -ge 3 ] && [ \"$1\" = push ] && [ \"$2\" = origin ] &&\n\
             \x20  [ \"$3\" = HEAD:refs/heads/main ] && [ ! -e \"$RACE_MARKER\" ]; then\n\
             \x20 : >\"$RACE_MARKER\"\n\
             \x20 \"$REAL_GIT\" -C \"$RACE_WORK\" push --quiet origin main\n\
             fi\n\
             exec \"$REAL_GIT\" \"$@\"\n",
        )?;
        Ok(real_git)
    }
}

/// Stale pins advance every gitlink and every lock node together.
fn stale_pins_advance_together(fx: &Fixture) -> Result<()> {
    fx.run_worker(&[])?;
    fx.assert_pinned_at_tips(
        "direct update did not move both the gitlink and lock to the source's main",
    )
}

/// A current pin is a true no-op: no commit lands on `ix/main`.
fn current_pins_are_a_noop(fx: &Fixture) -> Result<()> {
    let before = fx.ix_rev("main")?;
    fx.run_worker(&[])?;
    let after = fx.ix_rev("main")?;
    if before != after {
        bail!("current-pin run created an unexpected commit ({before} -> {after})");
    }
    Ok(())
}

/// A push that loses a race against an unrelated `ix/main` commit retries onto
/// the new tip, keeping the other commit rather than clobbering it, and leaves
/// the source it had no reason to move exactly where it was.
fn lost_race_retries_onto_new_tip(fx: &mut Fixture) -> Result<()> {
    fx.advance_source("index", "newer")?;

    let racer = fx.stage_racer_commit()?;
    let real_git = fx.install_racing_git_shim()?;
    let marker = fx.tmp.join("race-fired");
    fx.run_worker(&[
        ("RACE_MARKER", utf8(&marker)?),
        ("RACE_WORK", utf8(&racer)?),
        ("REAL_GIT", &real_git),
    ])?;

    if !marker.exists() {
        bail!(
            "race was never injected -- the shim's push match is stale, so this run proved nothing"
        );
    }
    fx.assert_pinned_at_tips("race retry did not converge on the source's main")?;

    let race_file = fx.ix_show("race.txt")?;
    if race_file.trim() != "preserve me" {
        bail!("race retry lost the concurrent ix/main commit (race.txt={race_file:?})");
    }
    Ok(())
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
    let mut fx = Fixture::new(tempdir.path().to_path_buf(), &workflow)?;

    stale_pins_advance_together(&fx)?;
    current_pins_are_a_noop(&fx)?;
    lost_race_retries_onto_new_tip(&mut fx)?;

    println!("submodule-sync-test: PASS");
    Ok(())
}
