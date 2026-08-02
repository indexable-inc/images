#![allow(
    dead_code,
    reason = "each integration-test binary compiles this module separately and uses a subset"
)]

//! Shared harness for the binary integration tests: stub tools on PATH plus
//! REAL local git repos standing in for the upstream and the fork repo. The
//! binaries only ever speak https URLs (the mapping's upstreamUrl and the
//! forkRepo-derived push URL), so the harness redirects those to the local
//! fixtures with `url.<dir>.insteadOf` in a scratch global gitconfig; no
//! network is touched.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Write an executable `#!/bin/sh` stub named `name` into `dir`.
pub fn write_stub(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// PATH with `stub_dir` prepended.
pub fn stub_path(stub_dir: &Path) -> String {
    format!("{}:{}", stub_dir.display(), std::env::var("PATH").unwrap())
}

pub struct Run {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run a crate binary with `args` in `cwd` under extra `envs`.
pub fn run_bin(exe: &str, args: &[&str], cwd: &Path, envs: &[(&str, String)]) -> Run {
    let mut command = Command::new(exe);
    command.args(args).current_dir(cwd);
    for (key, value) in envs {
        command.env(key, value);
    }
    let out = command.output().unwrap();
    Run {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Run git with a fixed test identity, asserting success.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed in {}: {}",
        cwd.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A local upstream repo with `main` as its default branch and one base
/// commit.
fn init_upstream(root: &Path) -> PathBuf {
    let upstream = root.join("upstream");
    fs::create_dir_all(&upstream).unwrap();
    git(&upstream, &["init", "--quiet", "-b", "main"]);
    fs::write(upstream.join("README"), "hello\n").unwrap();
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "--quiet", "-m", "base"]);
    upstream
}

/// The https URLs the binaries see; the gitconfig redirects them locally.
pub const UPSTREAM_URL: &str = "https://github.com/fakeorg/fakerepo.git";
pub const FORK_REPO: &str = "fakefork/fakerepo";
pub const FORK_URL: &str = "https://github.com/fakefork/fakerepo.git";

/// The default single test patch.
pub const SUBJECT: &str = "fakefix: repair the frobnicator widget alignment";
pub const BODY: &str = "This explains why the widget needed repairing.";

/// Local upstream + fork repos in the jj megamerge layout: the fork's
/// `ix-patched` branch holds a linear chain of patch commits on the upstream
/// base, sealed by an "ix megamerge" commit (tree = head tree, parent =
/// head), exactly the shape the migration pushes.
pub struct Fixture {
    pub upstream: PathBuf,
    pub fork: PathBuf,
    /// Scratch global gitconfig with the insteadOf redirects.
    pub gitconfig: PathBuf,
}

impl Fixture {
    /// Build the fixture under `root`. Each patch is `(subject, body)`; an
    /// empty body makes a reason-less commit (for the refusal tests).
    pub fn new(root: &Path, patches: &[(&str, &str)]) -> Self {
        let upstream = init_upstream(root);
        Self::seal(root, &upstream, "main", patches)
    }

    /// The #4038 shape: the fork is based on an upstream MAINTENANCE branch
    /// whose tip is not an ancestor of the (diverged) default branch, and
    /// the maintenance commits share a subject ("Bump version", exactly
    /// NixOS/nix's 2.34-maintenance). A series read anchored on the default
    /// branch merge-bases at the fork point and drags those commits into
    /// the series; only an `upstreamRef`-anchored read sees the fork
    /// patches alone.
    pub fn on_maintenance_branch(root: &Path, patches: &[(&str, &str)]) -> Self {
        let upstream = init_upstream(root);
        git(&upstream, &["checkout", "--quiet", "-b", "maintenance"]);
        for point in ["2.34.6", "2.34.7"] {
            fs::write(upstream.join("version"), format!("{point}\n")).unwrap();
            git(&upstream, &["add", "."]);
            git(&upstream, &["commit", "--quiet", "-m", "Bump version"]);
        }
        git(&upstream, &["checkout", "--quiet", "main"]);
        fs::write(upstream.join("feature"), "development moved on\n").unwrap();
        git(&upstream, &["add", "."]);
        git(
            &upstream,
            &["commit", "--quiet", "-m", "diverge the default branch"],
        );
        Self::seal(root, &upstream, "origin/maintenance", patches)
    }

    /// Clone the fork, commit the patch series on `base_ref`, and seal it
    /// with the megamerge commit under the `ix-patched` bookmark.
    fn seal(root: &Path, upstream: &Path, base_ref: &str, patches: &[(&str, &str)]) -> Self {
        let fork = root.join("fork");
        git(root, &["clone", "--quiet", upstream.to_str().unwrap(), "fork"]);
        git(&fork, &["checkout", "--quiet", "-b", "series", base_ref]);
        for (i, (subject, body)) in patches.iter().enumerate() {
            fs::write(fork.join(format!("patch-{i}.txt")), format!("{subject}\n")).unwrap();
            git(&fork, &["add", "."]);
            let message = if body.is_empty() {
                (*subject).to_owned()
            } else {
                format!("{subject}\n\n{body}")
            };
            git(&fork, &["commit", "--quiet", "-m", &message]);
        }
        // The megamerge seal: same tree as the series head, parent(s) = the
        // DAG head(s). Series readers must skip it.
        let head = git(&fork, &["rev-parse", "HEAD"]);
        #[expect(
            clippy::literal_string_with_formatting_args,
            reason = "^{tree} is git revision syntax, not a format placeholder"
        )]
        let tree = git(&fork, &["rev-parse", "HEAD^{tree}"]);
        let seal = git(
            &fork,
            &[
                "commit-tree",
                &tree,
                "-p",
                &head,
                "-m",
                &format!("ix megamerge: {} patches on {}", patches.len(), &head[..12]),
            ],
        );
        git(&fork, &["branch", "ix-patched", &seal]);

        let gitconfig = root.join("gitconfig");
        fs::write(
            &gitconfig,
            format!(
                "[url \"{}\"]\n\tinsteadOf = {UPSTREAM_URL}\n[url \"{}\"]\n\tinsteadOf = {FORK_URL}\n",
                upstream.display(),
                fork.display()
            ),
        )
        .unwrap();
        Self {
            upstream: upstream.to_path_buf(),
            fork,
            gitconfig,
        }
    }

    /// The env pairs that make the binaries hit the local fixtures.
    #[expect(
        clippy::anonymous_tuple_return_type,
        reason = "process env pairs feed Command::envs at the only consumers; a named struct would be re-flattened immediately"
    )]
    pub fn envs(&self) -> Vec<(&'static str, String)> {
        vec![
            ("GIT_CONFIG_GLOBAL", self.gitconfig.display().to_string()),
            ("GIT_CONFIG_NOSYSTEM", "1".to_owned()),
        ]
    }

    /// The sha of the series commit with this subject, from the fork repo.
    pub fn sha_of(&self, subject: &str) -> String {
        git(
            &self.fork,
            &[
                "log",
                "ix-patched",
                &format!("--grep=^{subject}$"),
                "--format=%H",
                "-1",
            ],
        )
    }
}

/// A v2 mapping entry as JSON (subject-keyed intent).
pub fn mapping_json(name: &str, patches_json: &str) -> String {
    mapping_json_on(name, None, patches_json)
}

/// Like [`mapping_json`], optionally declaring the upstream branch the fork
/// tracks (`upstreamRef`) instead of the upstream's default branch.
pub fn mapping_json_on(name: &str, upstream_ref: Option<&str>, patches_json: &str) -> String {
    let upstream_ref =
        upstream_ref.map_or_else(String::new, |r| format!(r#""upstreamRef":"{r}","#));
    format!(
        r#"[{{"name":"{name}","input":"{name}-src","forkRepo":"{FORK_REPO}","bookmark":"ix-patched",
  "upstreamUrl":"{UPSTREAM_URL}",{upstream_ref}"autoUpdate":false,
  "upstreamPolicy":{{"prsWelcome":true,"aiPrsAllowed":"unknown","citation":"https://example.com","notes":"t"}},
  "patches":{patches_json}}}]"#
    )
}

/// Read a fork's status file (packages/upstream-sync/status/<name>.json
/// under `root`, the test's working directory) as JSON.
pub fn status_json(root: &Path, name: &str) -> serde_json::Value {
    let raw = fs::read_to_string(
        root.join("packages/upstream-sync/status")
            .join(format!("{name}.json")),
    )
    .unwrap();
    serde_json::from_str(&raw).unwrap()
}
