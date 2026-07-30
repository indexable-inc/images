//! End-to-end tests against the real binary. These exist to pin the JSON
//! contract, key for key: an Elixir wrapper is built against that shape, so a
//! renamed or dropped key is a break, not a refactor.
//!
//! Every invocation passes `--dir`, so no test can read the ambient repo's
//! `.memories` or the home directory's.

use serde_json::Value;
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const BINARY: &str = env!("CARGO_BIN_EXE_memories");

/// The full key set of a `search` hit. Kept as a literal list rather than
/// derived from the struct, so a change to the struct has to be a deliberate
/// change here too.
const HIT_KEYS: [&str; 18] = [
    "slug",
    "path",
    "root",
    "tldr",
    "genre",
    "topic",
    "handle",
    "prior",
    "related",
    "supersedes",
    "scope",
    "bm25",
    "score",
    "stale",
    "stale_reason",
    "refuted",
    "validated",
    "body",
];

struct Repo {
    dir: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::create_dir_all(dir.path().join(".memories")).expect("a `.memories` directory");
        Self { dir }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn memory(&self, slug: &str, contents: &str) {
        std::fs::write(self.memory_path(slug), contents).expect("writing a memory");
    }

    /// A memory in a grouping subdirectory, one level down.
    fn group_memory(&self, group: &str, slug: &str, contents: &str) {
        let dir = self.path().join(".memories").join(group);
        std::fs::create_dir_all(&dir).expect("a group directory");
        std::fs::write(dir.join(format!("{slug}.md")), contents).expect("writing a memory");
    }

    fn file(&self, relative: &str, contents: &str) {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("a parent directory");
        }
        std::fs::write(path, contents).expect("writing a repo file");
    }

    fn memory_path(&self, slug: &str) -> PathBuf {
        self.path().join(".memories").join(format!("{slug}.md"))
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|error| panic!("stdout must be JSON ({error}):\n{}", self.stdout))
    }
}

fn run(args: &[&str]) -> Run {
    run_with_stdin(args, "")
}

fn run_with_stdin(args: &[&str], stdin: &str) -> Run {
    let mut child = Command::new(BINARY)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning the memories binary");
    child
        .stdin
        .as_mut()
        .expect("a piped stdin")
        .write_all(stdin.as_bytes())
        .expect("writing stdin");
    let output = child.wait_with_output().expect("waiting for the binary");
    Run {
        code: output.status.code().expect("an exit code"),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// The object's keys, sorted. `serde_json` parses an object into a sorted map,
/// so the order a caller sees on the wire is not recoverable here; that is what
/// [`assert_key_order`] is for.
fn keys(value: &Value) -> Vec<String> {
    let mut keys: Vec<String> = value
        .as_object()
        .expect("a JSON object")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

fn sorted(keys: &[&str]) -> Vec<String> {
    let mut owned: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
    owned.sort();
    owned
}

/// Assert the emitted bytes carry `keys` in the contract's order. Order is not
/// semantically meaningful in JSON, but a diff of this output is read by people,
/// and the contract prints them in a deliberate order.
fn assert_key_order(json: &str, keys: &[&str]) {
    // No key name is shared between the envelope and the objects inside it, so
    // scanning the whole document finds each one exactly where it belongs.
    let mut cursor = 0usize;
    for key in keys {
        let needle = format!("\"{key}\":");
        let at = json[cursor..].find(&needle).unwrap_or_else(|| {
            panic!("{needle} missing or out of contract order after byte {cursor}:\n{json}")
        });
        cursor += at + needle.len();
    }
}

fn hits(value: &Value) -> &Vec<Value> {
    value["hits"].as_array().expect("hits is an array")
}

fn slugs(value: &Value) -> Vec<String> {
    hits(value)
        .iter()
        .map(|hit| hit["slug"].as_str().expect("a slug").to_owned())
        .collect()
}

/// A validated, current memory; a refuted one; and one whose `based_on` moved.
fn three_fixtures() -> Repo {
    let repo = Repo::new();
    repo.file("src/rank.rs", "fn rank() {}\n");

    repo.memory(
        "nix-rebuild-cascade",
        concat!(
            "---\n",
            "tldr: An env var holding a store path makes every dependent rebuild\n",
            "genre: memory\n",
            "topic: [nix, builds]\n",
            "handle: [nix-dag, drvPath]\n",
            "prior: 0.8\n",
            "validated:\n",
            "  - at: 2026-07-20T18:22:11Z\n",
            "    by: claude-opus-5\n",
            "    how: nix-dag rebuild count\n",
            "    ok: true\n",
            "---\n",
            "A node thousands of derivations depend on is normal.\n",
        ),
    );

    repo.memory(
        "rebuild-is-always-the-lockfile",
        concat!(
            "---\n",
            "tldr: Every rebuild cascade comes from the lockfile\n",
            "genre: memory\n",
            "topic: [nix]\n",
            "validated:\n",
            "  - at: 2026-07-01T00:00:00Z\n",
            "    by: someone\n",
            "    how: believed it\n",
            "    ok: true\n",
            "  - at: 2026-07-21T00:00:00Z\n",
            "    by: claude-opus-5\n",
            "    how: counted the drvs; it was an env var\n",
            "    ok: false\n",
            "---\n",
            "Refuted: the lockfile was not the cause.\n",
        ),
    );

    repo.memory(
        "rank-on-sole-count",
        concat!(
            "---\n",
            "tldr: Rank a rebuild on sole count, not fan-out\n",
            "genre: memory\n",
            "topic: [nix]\n",
            "based_on:\n",
            "  - path: src/rank.rs\n",
            "    blake3: 0000000000000000\n",
            "validated:\n",
            "  - at: 2026-07-22T00:00:00Z\n",
            "    by: claude-opus-5\n",
            "    how: read the ranking code\n",
            "    ok: true\n",
            "---\n",
            "Sole count is the number of nodes that depend on this one alone.\n",
        ),
    );

    repo
}

#[test]
fn search_json_carries_exactly_the_contract_keys() {
    let repo = three_fixtures();
    let run = run(&[
        "--dir",
        &repo.path().display().to_string(),
        "--json",
        "search",
        "rebuild",
    ]);
    assert_eq!(run.code, 0, "stderr:\n{}", run.stderr);

    let output = run.json();
    assert_eq!(
        keys(&output),
        sorted(&["query", "roots", "scanned", "elapsed_ms", "hits"]),
        "top-level shape changed"
    );
    assert_key_order(
        &run.stdout,
        &["query", "roots", "scanned", "elapsed_ms", "hits"],
    );
    // A row per root, not a path: a caller with zero hits has to be able to
    // tell "the roots I expected, and they hold nothing" from "a root set that
    // resolved somewhere unexpected", and a bare path cannot say either.
    assert_eq!(
        output["roots"],
        serde_json::json!([{
            "path": repo.path().join(".memories").display().to_string(),
            "exists": true,
            "memories": 3,
        }]),
        "every result says which directories it read, and what each held"
    );
    assert_eq!(output["scanned"], 3, "three memory files were read");

    let hit = &hits(&output)[0];
    assert_eq!(keys(hit), sorted(&HIT_KEYS), "hit shape changed");
    assert_key_order(&run.stdout, &HIT_KEYS);
    assert!(
        hit["bm25"].as_f64().is_some_and(|bm25| bm25 > 0.0),
        "a hit carries its raw BM25: {hit}"
    );
    assert!(hit["score"].as_f64().is_some_and(|score| score > 0.0));
    assert_eq!(hit["root"], repo.path().display().to_string());
}

#[test]
fn refuted_memories_are_excluded_from_search_and_listed_by_refuted() {
    let repo = three_fixtures();
    let dir = repo.path().display().to_string();

    let default = run(&["--dir", &dir, "--json", "search", "rebuild"]).json();
    assert!(
        !slugs(&default).contains(&"rebuild-is-always-the-lockfile".to_owned()),
        "a refuted memory must not surface: {:?}",
        slugs(&default)
    );

    let all = run(&["--dir", &dir, "--json", "search", "rebuild", "--all"]).json();
    assert!(
        slugs(&all).contains(&"rebuild-is-always-the-lockfile".to_owned()),
        "--all includes it: {:?}",
        slugs(&all)
    );
    let refuted_hit = hits(&all)
        .iter()
        .find(|hit| hit["slug"] == "rebuild-is-always-the-lockfile")
        .expect("the refuted hit");
    assert_eq!(refuted_hit["refuted"], true, "and it is flagged");

    let rows = run(&["--dir", &dir, "--json", "refuted"]).json();
    let rows = rows["rows"].as_array().expect("rows is an array");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(keys(&rows[0]), sorted(&["slug", "path", "tldr", "reason"]));
    assert!(
        rows[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("it was an env var")),
        "the reason quotes the refuting evidence: {rows:?}"
    );
}

#[test]
fn a_stale_memory_is_still_returned_by_search_and_flagged() {
    let repo = three_fixtures();
    let dir = repo.path().display().to_string();

    let output = run(&["--dir", &dir, "--json", "search", "sole count"]).json();
    let hit = hits(&output)
        .iter()
        .find(|hit| hit["slug"] == "rank-on-sole-count")
        .expect("a stale memory is not excluded, it is flagged");
    assert_eq!(hit["stale"], true);
    assert!(
        hit["stale_reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("src/rank.rs")),
        "the reason names the path: {hit}"
    );

    let rows = run(&["--dir", &dir, "--json", "stale"]).json();
    assert_eq!(rows["rows"].as_array().expect("rows").len(), 1);
}

#[test]
fn show_emits_a_hit_without_the_ranking_keys() {
    let repo = three_fixtures();
    let run = run(&[
        "--dir",
        &repo.path().display().to_string(),
        "--json",
        "show",
        "nix-rebuild-cascade",
    ]);
    assert_eq!(run.code, 0, "stderr:\n{}", run.stderr);

    let hit = run.json();
    let expected: Vec<&str> = HIT_KEYS
        .iter()
        .copied()
        .filter(|key| *key != "bm25" && *key != "score")
        .collect();
    assert_eq!(
        keys(&hit),
        sorted(&expected),
        "show omits only bm25 and score"
    );
    assert_key_order(&run.stdout, &expected);
    assert_eq!(hit["genre"], "memory");
    assert_eq!(hit["topic"], serde_json::json!(["nix", "builds"]));
    assert!(
        hit["body"]
            .as_str()
            .is_some_and(|body| body.contains("A node"))
    );
}

#[test]
fn an_unresolved_slug_exits_one() {
    let repo = three_fixtures();
    let run = run(&[
        "--dir",
        &repo.path().display().to_string(),
        "--json",
        "show",
        "no-such-memory",
    ]);
    assert_eq!(run.code, 1, "stdout:\n{}", run.stdout);
    assert!(
        run.stderr.contains("no-such-memory"),
        "stderr:\n{}",
        run.stderr
    );
}

#[test]
fn an_empty_query_is_a_usage_error() {
    let repo = three_fixtures();
    let run = run(&["--dir", &repo.path().display().to_string(), "search", "  "]);
    assert_eq!(run.code, 2, "stderr:\n{}", run.stderr);
}

#[test]
fn dir_overrides_the_default_root_set_and_repeats() {
    let first = Repo::new();
    first.memory(
        "shared-slug",
        "---\ntldr: The first root's copy of the lesson\ngenre: living\n---\nFirst body.\n",
    );
    let second = Repo::new();
    second.memory(
        "shared-slug",
        "---\ntldr: The second root's copy of the lesson\ngenre: living\n---\nSecond body.\n",
    );

    let only_first = run(&[
        "--dir",
        &first.path().display().to_string(),
        "--json",
        "search",
        "lesson",
    ])
    .json();
    assert_eq!(only_first["scanned"], 1, "one --dir means one directory");

    let both = run(&[
        "--dir",
        &first.path().display().to_string(),
        "--dir",
        &second.path().display().to_string(),
        "--json",
        "search",
        "lesson",
    ])
    .json();
    assert_eq!(both["scanned"], 2, "two --dir values merge");
    assert_eq!(
        slugs(&both),
        ["shared-slug", "shared-slug"],
        "the same slug in two roots is two hits"
    );
    let roots: Vec<&str> = hits(&both)
        .iter()
        .map(|hit| hit["root"].as_str().expect("a root"))
        .collect();
    assert_ne!(roots[0], roots[1], "distinguished by root: {roots:?}");
}

#[test]
fn a_dir_with_no_memories_directory_is_an_error_rather_than_an_empty_result() {
    let empty = tempfile::tempdir().expect("a temp dir");
    let run = run(&[
        "--dir",
        &empty.path().display().to_string(),
        "--json",
        "search",
        "anything",
    ]);
    assert_eq!(run.code, 1, "stdout:\n{}", run.stdout);
    assert!(
        run.stderr.contains(&empty.path().display().to_string())
            && run.stderr.contains(".memories"),
        "the caller named it, so the message names it back: {}",
        run.stderr
    );
}

#[test]
fn remember_then_show_returns_what_was_written_and_passes_lint() {
    let repo = Repo::new();
    repo.file("src/rank.rs", "fn rank() {}\n");
    let dir = repo.path().display().to_string();

    let written = run_with_stdin(
        &[
            "--dir",
            &dir,
            "remember",
            "nix-rebuild-cascade",
            "--tldr",
            "it does not block: it launches more work",
            "--topic",
            "nix",
            "--handle",
            "nix-dag",
            "--prior",
            "0.8",
            "--based-on",
            "src/rank.rs",
        ],
        "The body, on stdin.\n",
    );
    assert_eq!(written.code, 0, "stderr:\n{}", written.stderr);
    assert!(
        repo.memory_path("nix-rebuild-cascade").is_file(),
        "stdout:\n{}",
        written.stdout
    );

    let shown = run(&["--dir", &dir, "--json", "show", "nix-rebuild-cascade"]).json();
    assert_eq!(shown["tldr"], "it does not block: it launches more work");
    assert_eq!(shown["topic"], serde_json::json!(["nix"]));
    assert_eq!(shown["handle"], serde_json::json!(["nix-dag"]));
    assert_eq!(shown["prior"], 0.8);
    assert_eq!(shown["body"], "The body, on stdin.\n");
    assert_eq!(shown["stale"], false, "the hash was recorded on write");

    // A just-written memory has no validation, which is `memory-unchecked` and
    // nothing else. Validating it must leave the file clean.
    let validated = run(&[
        "--dir",
        &dir,
        "validate",
        "nix-rebuild-cascade",
        "--by",
        "claude-opus-5",
        "--how",
        "ran the test",
    ]);
    assert_eq!(validated.code, 0, "stderr:\n{}", validated.stderr);

    let linted = run(&["--dir", &dir, "--json", "lint"]);
    assert_eq!(
        linted.code, 0,
        "a file written by remember and validated must lint clean:\n{}",
        linted.stdout
    );
    let output = linted.json();
    assert_eq!(keys(&output), sorted(&["diagnostics", "errors", "checked"]));
    assert_eq!(output["errors"], 0);
    assert_eq!(output["checked"], 1);
}

#[test]
fn remember_refuses_a_slug_that_is_not_kebab_case() {
    let repo = Repo::new();
    let run = run_with_stdin(
        &[
            "--dir",
            &repo.path().display().to_string(),
            "remember",
            "Not_Kebab",
            "--tldr",
            "A line",
        ],
        "Body.\n",
    );
    assert_eq!(run.code, 2, "stdout:\n{}", run.stdout);
}

#[test]
fn lint_reports_a_diagnostic_per_broken_rule_and_exits_one() {
    let repo = Repo::new();
    repo.memory(
        "broken",
        "---\ntldr: A line\nrelated: [nowhere]\n---\nBody.\n",
    );
    let run = run(&[
        "--dir",
        &repo.path().display().to_string(),
        "--json",
        "lint",
    ]);
    assert_eq!(run.code, 1, "a lint error exits 1:\n{}", run.stdout);

    let output = run.json();
    let diagnostics = output["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(
        keys(&diagnostics[0]),
        sorted(&["path", "line", "rule", "message"])
    );
    assert_key_order(&run.stdout, &["path", "line", "rule", "message"]);
    let rules: Vec<&str> = diagnostics
        .iter()
        .map(|diagnostic| diagnostic["rule"].as_str().expect("a rule"))
        .collect();
    assert_eq!(rules, ["memory-related-unresolved", "memory-unchecked"]);
    assert_eq!(output["errors"], 2);
}

#[test]
fn validate_clears_staleness_and_appends_to_the_history() {
    let repo = three_fixtures();
    let dir = repo.path().display().to_string();

    let before = run(&["--dir", &dir, "--json", "show", "rank-on-sole-count"]).json();
    assert_eq!(before["stale"], true);
    assert_eq!(before["validated"].as_array().expect("history").len(), 1);

    let validated = run(&[
        "--dir",
        &dir,
        "validate",
        "rank-on-sole-count",
        "--by",
        "claude-opus-5",
        "--how",
        "re-read src/rank.rs",
    ]);
    assert_eq!(validated.code, 0, "stderr:\n{}", validated.stderr);

    let after = run(&["--dir", &dir, "--json", "show", "rank-on-sole-count"]).json();
    assert_eq!(after["stale"], false, "validating rewrote the hash");
    assert_eq!(
        after["validated"].as_array().expect("history").len(),
        2,
        "appended rather than replaced"
    );
    let history = after["validated"].as_array().expect("history");
    assert_eq!(history[0]["by"], "claude-opus-5", "the old entry is first");
    assert_eq!(history[0]["at"], "2026-07-22T00:00:00Z");
    assert_eq!(keys(&history[1]), sorted(&["at", "by", "how", "ok"]));
}

#[test]
fn refute_records_the_refutation_and_names_the_successor() {
    let repo = three_fixtures();
    let dir = repo.path().display().to_string();

    let refuted = run(&[
        "--dir",
        &dir,
        "refute",
        "rank-on-sole-count",
        "--by",
        "claude-opus-5",
        "--how",
        "sole count double-counted shared nodes",
        "--instead",
        "nix-rebuild-cascade",
    ]);
    assert_eq!(refuted.code, 0, "stderr:\n{}", refuted.stderr);

    let target = run(&["--dir", &dir, "--json", "show", "rank-on-sole-count"]).json();
    assert_eq!(target["refuted"], true);

    let successor = run(&["--dir", &dir, "--json", "show", "nix-rebuild-cascade"]).json();
    assert_eq!(
        successor["supersedes"],
        serde_json::json!(["rank-on-sole-count"]),
        "`supersedes` lives on the successor"
    );
}

#[test]
fn a_malformed_file_is_reported_on_stderr_and_never_silently_skipped() {
    let repo = three_fixtures();
    repo.memory("broken", "no frontmatter at all\n");
    let dir = repo.path().display().to_string();

    let searched = run(&["--dir", &dir, "--json", "search", "rebuild"]);
    assert_eq!(searched.code, 0, "one bad file must not fail the search");
    assert!(
        searched.stderr.contains("broken.md") && searched.stderr.contains("memory-frontmatter"),
        "the JSON contract has no place for this, so it goes to stderr: {}",
        searched.stderr
    );
    assert_eq!(
        searched.json()["scanned"],
        4,
        "and it still counts as scanned"
    );
}

#[test]
fn roots_prints_the_resolved_root_set_and_matches_what_search_reports() {
    let first = Repo::new();
    first.memory(
        "one",
        "---\ntldr: The first root's lesson about rebuilds\ngenre: living\n---\nBody.\n",
    );
    let second = Repo::new();
    second.memory(
        "two",
        "---\ntldr: The second root's lesson about rebuilds\ngenre: living\n---\nBody.\n",
    );
    let args = [
        "--dir",
        &first.path().display().to_string(),
        "--dir",
        &second.path().display().to_string(),
    ]
    .map(String::from);

    let mut roots_args: Vec<&str> = args.iter().map(String::as_str).collect();
    roots_args.extend(["--json", "roots"]);
    let listed = run(&roots_args).json();
    assert_eq!(keys(&listed), ["roots"]);

    let mut search_args: Vec<&str> = args.iter().map(String::as_str).collect();
    search_args.extend(["--json", "search", "rebuilds"]);
    let searched = run(&search_args).json();
    assert_eq!(
        listed["roots"], searched["roots"],
        "two spellings of one root set is how they drift"
    );
    assert_eq!(
        listed["roots"],
        serde_json::json!([
            {
                "path": first.path().join(".memories").display().to_string(),
                "exists": true,
                "memories": 1,
            },
            {
                "path": second.path().join(".memories").display().to_string(),
                "exists": true,
                "memories": 1,
            },
        ]),
        "in precedence order, as `.memories` rows carrying their own counts"
    );
}

#[test]
fn a_memory_one_level_down_is_found_and_keeps_its_stem_as_its_slug() {
    let repo = Repo::new();
    repo.group_memory(
        "cas",
        "cas-gc-proof",
        "---\ntldr: Garbage collection proves the store is reachable\ngenre: living\n---\nBody.\n",
    );
    let dir = repo.path().display().to_string();

    let shown = run(&["--dir", &dir, "--json", "show", "cas-gc-proof"]);
    assert_eq!(
        shown.code, 0,
        "the slug is the stem, not the path: {}",
        shown.stderr
    );
    let hit = shown.json();
    assert_eq!(hit["slug"], "cas-gc-proof");
    assert_eq!(
        hit["root"],
        repo.path().display().to_string(),
        "the grouping directory is not a root"
    );

    let searched = run(&["--dir", &dir, "--json", "search", "garbage collection"]).json();
    assert_eq!(searched["scanned"], 1, "the nested file is scanned");
}

#[test]
fn a_nested_root_returns_hits_from_both_levels() {
    let repo = Repo::new();
    repo.memory(
        "top-level-lesson",
        "---\ntldr: A rebuild lesson at the top level\ngenre: living\n---\nBody.\n",
    );
    repo.group_memory(
        "cas",
        "nested-lesson",
        "---\ntldr: A rebuild lesson one level down\ngenre: living\n---\nBody.\n",
    );
    let output = run(&[
        "--dir",
        &repo.path().display().to_string(),
        "--json",
        "search",
        "rebuild lesson",
    ])
    .json();
    assert_eq!(output["scanned"], 2);
    assert_eq!(
        slugs(&output),
        ["nested-lesson", "top-level-lesson"],
        "both levels answer, and the slug is the stem either way: {output}"
    );
    assert_eq!(
        output["roots"][0]["memories"], 2,
        "counted across leaves: {output}"
    );
}

/// The same stem in two different roots is two memories, not a collision: the
/// collision rule is per root, because a root is where a slug has to be unique.
#[test]
fn the_same_stem_in_two_roots_is_two_memories_and_lints_clean() {
    let first = Repo::new();
    first.memory(
        "shared-stem",
        "---\ntldr: The first root's take on rebuilds\ngenre: living\n---\nBody.\n",
    );
    let second = Repo::new();
    second.memory(
        "shared-stem",
        "---\ntldr: The second root's take on rebuilds\ngenre: living\n---\nBody.\n",
    );
    let args = [
        "--dir".to_owned(),
        first.path().display().to_string(),
        "--dir".to_owned(),
        second.path().display().to_string(),
    ];

    let mut lint_args: Vec<&str> = args.iter().map(String::as_str).collect();
    lint_args.push("lint");
    let linted = run(&lint_args);
    assert_eq!(
        linted.code, 0,
        "one stem per root is fine:\n{}",
        linted.stdout
    );

    let mut search_args: Vec<&str> = args.iter().map(String::as_str).collect();
    search_args.extend(["--json", "search", "rebuilds"]);
    let output = run(&search_args).json();
    assert_eq!(slugs(&output), ["shared-stem", "shared-stem"]);
}

#[test]
fn a_query_with_no_good_answer_comes_back_empty() {
    let repo = three_fixtures();
    let searched = run(&[
        "--dir",
        &repo.path().display().to_string(),
        "--json",
        "search",
        "kubernetes ingress certificates",
    ]);
    assert_eq!(searched.code, 0);
    let output = searched.json();
    assert!(
        hits(&output).is_empty(),
        "below the score floor is worse than no answer: {output}"
    );
    assert_eq!(output["scanned"], 3, "and it still says what it read");
}

#[test]
fn remember_writes_a_user_scope_and_rejects_anything_else() {
    let repo = Repo::new();
    let dir = repo.path().display().to_string();

    let written = run_with_stdin(
        &[
            "--dir",
            &dir,
            "remember",
            "my-own-lesson",
            "--tldr",
            "Mine alone",
            "--scope",
            "user:andrewgazelka",
        ],
        "Body.\n",
    );
    assert_eq!(written.code, 0, "stderr:\n{}", written.stderr);
    let shown = run(&["--dir", &dir, "--json", "show", "my-own-lesson"]).json();
    assert_eq!(shown["scope"], "user:andrewgazelka");

    let rejected = run_with_stdin(
        &[
            "--dir",
            &dir,
            "remember",
            "another-lesson",
            "--tldr",
            "A line",
            "--scope",
            "everyone",
        ],
        "Body.\n",
    );
    assert_eq!(rejected.code, 2, "stdout:\n{}", rejected.stdout);
}

#[test]
fn a_retired_frontmatter_key_is_a_lint_error_of_its_own() {
    let repo = Repo::new();
    repo.memory(
        "was-always",
        "---\ntldr: A line\nalways: true\ngenre: living\n---\nBody.\n",
    );
    let run = run(&[
        "--dir",
        &repo.path().display().to_string(),
        "--json",
        "lint",
    ]);
    assert_eq!(run.code, 1);
    let output = run.json();
    let rules: Vec<&str> = output["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .map(|diagnostic| diagnostic["rule"].as_str().expect("a rule"))
        .collect();
    assert_eq!(rules, ["memory-unknown-key"], "{output}");
}

#[test]
fn lint_fix_sorts_lists_and_refreshes_hashes_without_touching_the_body() {
    let repo = Repo::new();
    repo.file("src/rank.rs", "fn rank() {}\n");
    repo.memory(
        "needs-a-fix",
        concat!(
            "---\n",
            "tldr: A line\n",
            "topic: [zeta, alpha]\n",
            "based_on:\n",
            "  - path: src/rank.rs\n",
            "    blake3: 0000000000000000\n",
            "genre: living\n",
            "---\n",
            "Body.\n",
        ),
    );
    let dir = repo.path().display().to_string();

    let fixed = run(&["--dir", &dir, "lint", "--fix"]);
    assert_eq!(
        fixed.code, 0,
        "stdout:\n{}\nstderr:\n{}",
        fixed.stdout, fixed.stderr
    );

    let after = std::fs::read_to_string(repo.memory_path("needs-a-fix")).expect("reading back");
    assert!(after.contains("topic: [alpha, zeta]"), "{after}");
    assert!(!after.contains("blake3: 0000000000000000"), "{after}");
    assert!(after.ends_with("---\nBody.\n"), "{after}");

    let again = run(&["--dir", &dir, "lint", "--fix"]);
    assert_eq!(
        again.stdout.trim(),
        "Errors: 0  Checked: 1",
        "a second --fix has nothing to do:\n{}",
        again.stdout
    );
}
