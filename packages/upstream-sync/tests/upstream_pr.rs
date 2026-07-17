//! End-to-end `upstream-pr` tests against a real local git upstream: the
//! closure/fetch/am/branch/preflight/compose mechanism runs for real (real
//! `git` on PATH via the workspace's packageTestInputs). `--dry-run` covers
//! the no-push validation surface; the `--open` tests stub `gh` and rewrite
//! the fork push URL to a local bare repo (`url.<dir>.insteadOf` in a
//! scratch HOME), so even the outward path runs sandboxed with no network.
//! The nu predecessor never had this coverage.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{run_bin, stub_path, write_stub};

/// gh stub for the --open path: the fork already "exists" (`repo view`
/// succeeds) and `pr create` records its argv for assertion, mimicking gh's
/// created-PR URL output line.
const GH_STUB: &str = r#"case "$1 $2" in
  "repo view") exit 0 ;;
  "pr create") printf '%s\n' "$@" > "$GH_PR_CREATE_ARGS"
    echo "https://github.com/fakeorg/upstream/pull/123" ;;
  *) echo "stub gh: unexpected: $*" >&2; exit 1 ;;
esac"#;

/// A PR template shaped like nushell's: the three sections this tool knows
/// how to fill, plus an intro comment the renderer must skip.
const TEMPLATE: &str = "<!-- intro comment -->\n## Description\n(explain what and why)\n\n## User-facing changes (Release notes)\n\n## Additional notes\n";

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A local "upstream" whose default branch is main (optionally shipping a PR
/// template), plus one commit on top exported as our fork patch (then
/// dropped from upstream, so the patch is genuinely ours).
struct Fixture {
    tmp: tempfile::TempDir,
    patch_name: String,
}

impl Fixture {
    fn root(&self) -> &Path {
        self.tmp.path()
    }

    fn upstream(&self) -> PathBuf {
        self.tmp.path().join("upstream")
    }

    /// Write the fork mapping and return its path as an arg string.
    /// `preflight` and `patches` are raw JSON fragments.
    fn write_mapping(&self, preflight: &str, patches: &str) -> String {
        let mapping = self.root().join("mapping.json");
        fs::write(
            &mapping,
            format!(
                r#"[{{"name":"fake","input":"fake-src","url":"file://{}","patchDir":"patches","autoUpdate":false,"preflight":{preflight},"patches":{patches}}}]"#,
                self.upstream().display()
            ),
        )
        .unwrap();
        mapping.display().to_string()
    }
}

fn setup_fixture(template: Option<&str>) -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let upstream = root.join("upstream");
    fs::create_dir(&upstream).unwrap();
    git(&upstream, &["init", "--quiet", "-b", "main"]);
    fs::write(upstream.join("README"), "hello\n").unwrap();
    if let Some(text) = template {
        fs::create_dir_all(upstream.join(".github")).unwrap();
        fs::write(upstream.join(".github/pull_request_template.md"), text).unwrap();
    }
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "--quiet", "-m", "base"]);
    fs::write(upstream.join("README"), "hello widget\n").unwrap();
    git(&upstream, &["add", "."]);
    git(
        &upstream,
        &[
            "commit",
            "--quiet",
            "-m",
            "fakefix: repair widget\n\nThis explains why.",
        ],
    );
    let patches = root.join("patches");
    fs::create_dir(&patches).unwrap();
    git(
        &upstream,
        &["format-patch", "-1", "-o", &patches.display().to_string()],
    );
    git(&upstream, &["reset", "--quiet", "--hard", "HEAD~1"]);
    let patch_name = fs::read_dir(&patches)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();
    fs::write(
        patches.join("dag.json"),
        format!(r#"{{"comment":"t","base":"x","nodes":[{{"patch":"{patch_name}","deps":[]}}]}}"#),
    )
    .unwrap();
    Fixture { tmp, patch_name }
}

fn base_envs() -> Vec<(&'static str, String)> {
    vec![
        ("PATH", std::env::var("PATH").unwrap()),
        ("GIT_COMMITTER_NAME", "Test".to_owned()),
        ("GIT_COMMITTER_EMAIL", "t@t".to_owned()),
    ]
}

/// The sandboxed --open harness: a stubbed `gh` (recording `pr create` argv
/// into `args_file`), a local bare repo standing in for the indexable-inc
/// fork, and a scratch HOME whose .gitconfig `insteadOf`-rewrites the fork
/// push URL onto it.
struct OpenHarness {
    envs: Vec<(&'static str, String)>,
    args_file: PathBuf,
}

fn setup_open(root: &Path) -> OpenHarness {
    let stubs = root.join("stubs");
    fs::create_dir(&stubs).unwrap();
    write_stub(&stubs, "gh", GH_STUB);
    fs::create_dir(root.join("forks")).unwrap();
    git(root, &["init", "--quiet", "--bare", "forks/upstream.git"]);
    fs::write(
        root.join(".gitconfig"),
        format!(
            "[url \"file://{}/forks/\"]\n\tinsteadOf = https://github.com/indexable-inc/\n",
            root.display()
        ),
    )
    .unwrap();
    let args_file = root.join("gh-pr-create-args.txt");
    let envs = vec![
        ("PATH", stub_path(&stubs)),
        ("HOME", root.display().to_string()),
        ("GH_PR_CREATE_ARGS", args_file.display().to_string()),
        ("GIT_COMMITTER_NAME", "Test".to_owned()),
        ("GIT_COMMITTER_EMAIL", "t@t".to_owned()),
    ];
    OpenHarness { envs, args_file }
}

/// The recorded `gh pr create` invocation: the flag lines before `--body`
/// and the body payload after it.
struct CreateArgs {
    flags: Vec<String>,
    body: String,
}

fn read_create_args(path: &Path) -> CreateArgs {
    let raw = fs::read_to_string(path).unwrap();
    let (flags, body) = raw.split_once("--body\n").unwrap();
    CreateArgs {
        flags: flags.lines().map(str::to_owned).collect(),
        body: body.strip_suffix('\n').unwrap().to_owned(),
    }
}

#[test]
fn dry_run_applies_closure_onto_upstream_tip() {
    let fixture = setup_fixture(None);
    let mapping = fixture.write_mapping("[]", "{}");
    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-pr"),
        &[
            "--dry-run",
            "--mapping",
            &mapping,
            "fake",
            &fixture.patch_name,
        ],
        fixture.root(),
        &base_envs(),
    );
    assert_eq!(
        run.status, 0,
        "dry run failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout.contains("upstream default branch is main"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("applied 1 commit(s) cleanly"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout
            .contains("--dry-run: would push branch upstream-pr/fake/fakefix-repair-widget"),
        "{}",
        run.stdout
    );
    // The dry run also composes and previews the PR content it would open
    // with (validate content without touching any remote).
    assert!(
        run.stdout
            .contains("with --open the PR would be titled \"fakefix: repair widget\""),
        "{}",
        run.stdout
    );

    // The scratch repo named in the output survives for inspection.
    let scratch = run
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("upstream-pr: scratch repo left for inspection: "))
        .unwrap()
        .to_owned();
    assert!(
        Path::new(&scratch).join(".git").exists(),
        "scratch repo missing: {scratch}"
    );
    fs::remove_dir_all(&scratch).unwrap();
}

#[test]
fn preflight_failure_aborts_loudly_before_any_push() {
    let fixture = setup_fixture(None);
    // The first command proves preflight runs in the PATCHED scratch tree;
    // the second fails so the contribution must abort naming it.
    let mapping = fixture.write_mapping(
        r#"["grep -q widget README","echo preflight-boom >&2; exit 4"]"#,
        "{}",
    );
    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-pr"),
        &[
            "--dry-run",
            "--mapping",
            &mapping,
            "fake",
            &fixture.patch_name,
        ],
        fixture.root(),
        &base_envs(),
    );
    assert_ne!(
        run.status, 0,
        "a red preflight must abort:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout
            .contains("preflight `grep -q widget README` passed"),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("preflight-boom"),
        "failing command output not surfaced: {}",
        run.stdout
    );
    assert!(
        run.stderr
            .contains("preflight `echo preflight-boom >&2; exit 4` FAILED in the patched checkout"),
        "{}",
        run.stderr
    );
}

#[test]
fn dry_run_refuses_template_without_release_notes() {
    let fixture = setup_fixture(Some(TEMPLATE));
    // The target template demands a release-notes section, but the patch
    // declares no `releaseNotes`: refuse loudly, even under --dry-run.
    let mapping = fixture.write_mapping("[]", "{}");
    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-pr"),
        &[
            "--dry-run",
            "--mapping",
            &mapping,
            "fake",
            &fixture.patch_name,
        ],
        fixture.root(),
        &base_envs(),
    );
    assert_ne!(
        run.status, 0,
        "an unfillable template must refuse:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stderr.contains("declares no `releaseNotes`")
            && run
                .stderr
                .contains("NOT opening a template-noncompliant PR"),
        "{}",
        run.stderr
    );
}

#[test]
fn open_renders_template_and_defaults_to_ready_for_review() {
    let fixture = setup_fixture(Some(TEMPLATE));
    let patch = &fixture.patch_name;
    let mapping = fixture.write_mapping(
        "[]",
        &format!(
            r#"{{"{patch}":{{"upstream":"attempt","reason":"t","prExtra":"Related issue: #7106.","releaseNotes":"Widgets improved."}}}}"#
        ),
    );
    let harness = setup_open(fixture.root());
    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-pr"),
        &["--open", "--mapping", &mapping, "fake", patch],
        fixture.root(),
        &harness.envs,
    );
    assert_eq!(
        run.status, 0,
        "open failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout.contains(
            "rendering the PR body into the target repo's template (.github/pull_request_template.md)"
        ),
        "{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("opening ready-for-review PR upstream"),
        "{}",
        run.stdout
    );

    // The branch really landed on the (redirected) fork remote.
    git(
        &fixture.root().join("forks/upstream.git"),
        &[
            "rev-parse",
            "--verify",
            "refs/heads/upstream-pr/fake/fakefix-repair-widget",
        ],
    );

    let created = read_create_args(&harness.args_file);
    assert!(
        !created.flags.iter().any(|f| f == "--draft"),
        "ready for review is the default: {:?}",
        created.flags
    );
    assert_eq!(
        created.body,
        format!(
            "## Description\n\nThis explains why.\n\n## User-facing changes (Release notes)\n\nWidgets improved.\n\n## Additional notes\n\nRelated issue: #7106.\n\n---\n\nContributed from a maintained fork patch series (patch {patch}).\n\nPrepared with AI assistance (Claude); directed and reviewed by a human maintainer."
        )
    );
}

#[test]
fn open_draft_keeps_the_plain_body_without_a_template() {
    let fixture = setup_fixture(None);
    let patch = &fixture.patch_name;
    let mapping = fixture.write_mapping(
        "[]",
        &format!(
            r#"{{"{patch}":{{"upstream":"attempt","reason":"t","prExtra":"Related issue: #7106."}}}}"#
        ),
    );
    let harness = setup_open(fixture.root());
    let run = run_bin(
        env!("CARGO_BIN_EXE_upstream-pr"),
        &["--open", "--draft", "--mapping", &mapping, "fake", patch],
        fixture.root(),
        &harness.envs,
    );
    assert_eq!(
        run.status, 0,
        "open failed:\n{}\n{}",
        run.stdout, run.stderr
    );
    assert!(
        run.stdout.contains("opening DRAFT PR upstream"),
        "{}",
        run.stdout
    );

    let created = read_create_args(&harness.args_file);
    assert!(
        created.flags.iter().any(|f| f == "--draft"),
        "--draft not forwarded: {:?}",
        created.flags
    );
    // No template: the plain composition (commit body + prExtra +
    // attribution) byte-for-byte, exactly as before template support.
    assert_eq!(
        created.body,
        format!(
            "This explains why.\n\nRelated issue: #7106.\n\n---\n\nContributed from a maintained fork patch series (patch {patch}).\n\nPrepared with AI assistance (Claude); directed and reviewed by a human maintainer."
        )
    );
}
