//! Execute a [`BenchSuite`] into [`Run`]s and record them.
//!
//! This is the seam that ties the harnesses, the schema, and the store
//! together: for each bench it gathers metrics (micro via the in-process
//! sampler, macro via the subprocess harness), stamps them with the machine id,
//! commit, dirty flag, and timestamp, and appends each [`Run`] to the store. The
//! CLI then compares each fresh run against its baseline.

use crate::schema::{Metric, Run, machine_id};
use crate::suite::BenchSuite;

/// Git context for a run: the resolved commit and whether the tree is dirty.
#[derive(Debug, Clone)]
pub struct GitContext {
    /// The commit the bench ran against, or `unknown` outside a repo.
    pub commit: String,
    /// Whether the working tree had uncommitted changes.
    pub dirty: bool,
}

impl GitContext {
    /// Resolve the git context for `repo` by shelling out to `git`. Outside a
    /// repository this returns `commit = "unknown", dirty = false` rather than
    /// erroring, so a bench can still run and record (just without a
    /// commit-keyed baseline).
    #[must_use]
    pub fn resolve(repo: &std::path::Path) -> Self {
        let commit = git_output(repo, &["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_owned());
        // `status --porcelain` prints one line per changed path; empty means
        // clean. A failure (not a repo) is reported as clean alongside the
        // `unknown` commit.
        let dirty = git_output(repo, &["status", "--porcelain"]).is_some_and(|s| !s.trim().is_empty());
        Self { commit, dirty }
    }
}

/// Run a `git` command in `repo`, returning trimmed stdout on success.
fn git_output(repo: &std::path::Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").arg("-C").arg(repo).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Execute every bench in `suite` and return one [`Run`] per bench.
///
/// Micro benches run first (cheap, in-process), then macro benches. A macro
/// bench that fails (non-zero exit, malformed `@bench` line) aborts the whole
/// invocation with that error, because a partial suite would record a misleading
/// baseline for the benches that did run.
///
/// # Errors
///
/// Propagates the machine-id failure and any macro-harness failure.
pub fn execute(suite: &mut BenchSuite<'_>, git: &GitContext) -> crate::Result<Vec<Run>> {
    let machine = machine_id()?;
    let timestamp = chrono::Utc::now().timestamp();

    let mut runs = Vec::with_capacity(suite.micro.len() + suite.macros.len());

    for bench in &mut suite.micro {
        let metrics = crate::micro::bench_fn(bench.config, &mut bench.body);
        runs.push(build_run(&suite.name, &bench.name, metrics, &machine, git, timestamp));
    }

    for bench in &suite.macros {
        let metrics = crate::macro_harness::run_command(&bench.program, &bench.args, bench.runs)?;
        runs.push(build_run(&suite.name, &bench.name, metrics, &machine, git, timestamp));
    }

    Ok(runs)
}

/// Assemble one [`Run`] from collected metrics and the shared run context.
fn build_run(suite: &str, bench: &str, metrics: Vec<Metric>, machine: &str, git: &GitContext, timestamp: i64) -> Run {
    Run {
        suite: suite.to_owned(),
        bench: bench.to_owned(),
        metrics,
        machine_id: machine.to_owned(),
        git_commit: git.commit.clone(),
        git_dirty: git.dirty,
        timestamp_unix: timestamp,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{HistoryStore, LocalDirStore};
    use crate::suite::{BenchSuite, MacroBench, MicroBench};

    #[test]
    fn executing_a_self_demo_suite_records_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = LocalDirStore::new(dir.path()).expect("store");

        let mut suite = BenchSuite::new("self-demo")
            .micro(MicroBench::new("noop", || {
                std::hint::black_box(2 + 2);
            }))
            .macro_bench(MacroBench::new("true", "true", Vec::<String>::new()).with_runs(2));

        let git = GitContext {
            commit: "test".to_owned(),
            dirty: false,
        };
        let runs = execute(&mut suite, &git).expect("execute");
        assert_eq!(runs.len(), 2, "one run per bench");

        for run in &runs {
            store.append(run).expect("record");
        }

        let micro = store.runs_for("self-demo", "noop").expect("read micro");
        assert_eq!(micro.len(), 1);
        assert!(micro[0].metric("wall_clock").is_some(), "micro bench records wall_clock");

        let mac = store.runs_for("self-demo", "true").expect("read macro");
        assert_eq!(mac.len(), 1);
        assert!(mac[0].metric("max_rss").is_some(), "macro bench records max_rss");
    }
}
