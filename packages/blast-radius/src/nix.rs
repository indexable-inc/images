//! Nix subprocess layer: evaluate `.#checks.x86_64-linux` at a revision and read
//! derivation graphs out of the store.
//!
//! Both are import-from-derivation heavy (the per-unit Cargo graph is rendered
//! by `nix-cargo-unit`, x86_64-linux only), so an end-to-end run needs a Linux
//! builder; `nix-eval-jobs` keeps evaluation memory bounded the way the old
//! nushell tool did.

use std::collections::BTreeMap;
use std::process::Command;

use color_eyre::eyre::{Context, Result, bail};
use serde::Deserialize;

use crate::causes::{DrvNode, Graph};

/// Pinned `nix-eval-jobs` so evaluation behavior does not drift with the user's
/// channels. Matches the revision the old nushell tool used.
const EVAL_JOBS: &str =
    "github:nix-community/nix-eval-jobs/65ebf5b7cd453a27af09cf02b1fc57b3568cc4b7";

/// One evaluated check: its attribute name and the derivation it builds.
pub struct Check {
    pub attr: String,
    pub drv_path: String,
}

/// An attr that failed to evaluate, with the nix-eval-jobs error. Carried (not
/// just the name) so a fail-closed bail on a head regression can print WHY it
/// failed: per-attr eval failures exit nix-eval-jobs 0, so the error text is
/// otherwise never surfaced in CI logs.
#[derive(Debug, PartialEq, Eq)]
pub struct EvalFailure {
    pub attr: String,
    pub error: String,
}

/// The result of evaluating `.#checks.x86_64-linux` at one rev: the buildable
/// checks, plus the attrs that failed to evaluate there (no derivation, so not a
/// rebuild target). The caller diffs `failures` across base and head to tell a
/// pre-existing catalog failure (tolerated) from one this change introduced.
pub struct EvalResult {
    pub checks: Vec<Check>,
    pub failures: Vec<EvalFailure>,
}

/// One line of `nix-eval-jobs` output.
#[derive(Deserialize)]
struct EvalRow {
    attr: String,
    #[serde(default)]
    #[serde(rename = "drvPath")]
    drv_path: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// The `.drv` basename of a store path (the segment after the last `/`). This is
/// the key `nix derivation show` uses for derivations and their inputs, and it is
/// input-addressed, so an identical basename means an identical derivation.
pub fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Strip a store path to its derivation name: drop the `/nix/store/<hash>-`
/// prefix and the `.drv` suffix, leaving e.g. `ix-rust-workspace` or
/// `cargo-unit-source-tui-0.1.0-<hash>` -- a stable, readable label.
pub fn drv_name(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let base = base.strip_suffix(".drv").unwrap_or(base);
    let bytes = base.as_bytes();
    let has_hash_prefix = bytes.len() > 33
        && bytes[32] == b'-'
        && bytes[..32]
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    if has_hash_prefix {
        base[33..].to_owned()
    } else {
        base.to_owned()
    }
}

/// Run a command, returning stdout on success and a stderr-bearing error on
/// failure. Never swallows stderr: a nonzero exit carries the real reason.
fn run(command: &mut Command) -> Result<String> {
    let output = command
        .output()
        .with_context(|| format!("spawn {command:?}"))?;
    if !output.status.success() {
        bail!(
            "{command:?} failed ({}):\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout).context("command stdout was not UTF-8")
}

/// Does `rev` expose the sharded `ciChecks` flake output? Probes cheaply: the
/// `--apply builtins.isAttrs` forces only the `{ <system> = ...; }` output spine,
/// not the catalog under it.
fn has_ci_checks(repo: &str, rev: &str) -> bool {
    let flakeref = format!("git+file://{repo}?rev={rev}&allRefs=1#ciChecks");
    Command::new("nix")
        .args([
            "eval",
            &flakeref,
            "--apply",
            "builtins.isAttrs",
            "--option",
            "accept-flake-config",
            "true",
        ])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// The catalog output to diff, chosen ONCE for both revisions so base and head
/// are keyed identically.
///
/// Prefer the sharded `ciChecks` (see [`eval_checks`]), but blast-radius diffs
/// head against the merge base, and that base can be a commit from before
/// `ciChecks` existed. If either revision lacks it, fall back to the flat
/// `checks` for BOTH. Choosing per revision would key the same derivation as
/// `rust-foo-package` at a flat base and `rust-foo.package` at a sharded head;
/// the diff keys by attr name, so every unchanged derivation would read as
/// removed+added and skip root-cause analysis. Migration shim: once no evaluated
/// base predates `ciChecks`, drop the probe and target `ciChecks` directly
/// (ENG-2201).
pub fn catalog_attr(repo: &str, base: &str, head: &str) -> &'static str {
    if has_ci_checks(repo, base) && has_ci_checks(repo, head) {
        "ciChecks"
    } else {
        "checks"
    }
}

/// Evaluate every check derivation at `rev` of the local repo, reading the
/// catalog from flake output `attr` (`ciChecks` or `checks`; see
/// [`catalog_attr`]).
///
/// `ciChecks` keys each crate's per-#[test] checks under a `recurseForDerivations`
/// group, so `nix-eval-jobs` enumerates cheap per-package names at the root and
/// forces each crate's manifest IFD in its own worker job. The flat `checks`
/// would force every crate's manifest in the single worker assigned the root
/// attrpath, ballooning it to tens of GiB and getting it earlyoom-killed on the
/// shared CI host (ENG-2201). Both outputs hold the same leaf derivations, so the
/// per-#[test] diff is identical as long as base and head use the same `attr`.
///
/// `nix-eval-jobs` sits at the head of the pipeline; a startup/lock/fetch
/// failure surfaces here rather than yielding an empty set that silently
/// under-reports the blast radius.
pub fn eval_checks(repo: &str, rev: &str, attr: &str) -> Result<EvalResult> {
    let flakeref = format!("git+file://{repo}?rev={rev}&allRefs=1#{attr}.x86_64-linux");
    let stdout = run(Command::new("nix").args([
        "run",
        EVAL_JOBS,
        "--",
        "--flake",
        &flakeref,
        // 4, not 8: main runs the base and head evals concurrently, so the two
        // nix-eval-jobs processes are alive at once. 4 workers each keeps the
        // peak at 8 evaluator heaps -- the same footprint as the old single
        // 8-worker eval -- so the parallelism does not double memory. Running
        // 16 evaluators alongside a full flake-check build OOM-killed the
        // shared runner (both jobs died together mid-eval).
        "--workers",
        "4",
        "--option",
        "accept-flake-config",
        "true",
        // The base eval predates any `nixConfig` declaration, so enable the
        // content-addressed feature directly rather than via the flake config.
        "--option",
        "extra-experimental-features",
        "ca-derivations",
        // A stale eval cache would mask a real rebuild; force a fresh eval.
        "--option",
        "eval-cache",
        "false",
    ]))
    .with_context(|| format!("evaluate checks at {rev}"))?;

    let Partitioned {
        checks,
        mut eval_failures,
        unexpected,
    } = partition_eval_rows(&stdout)?;

    // Neither a drvPath nor an error is a contract violation of nix-eval-jobs
    // (every row carries one or the other); fail loudly rather than guess at a
    // shape that could silently under-report the blast radius.
    if !unexpected.is_empty() {
        bail!(
            "checks at {rev} produced {} row(s) with neither drvPath nor error: {}",
            unexpected.len(),
            unexpected.join(", ")
        );
    }

    // Eval failures are returned, not skipped here: the caller distinguishes a
    // failure present at base (a pre-existing catalog issue, tolerated) from one
    // new at head (a regression this change introduced, which must fail closed).
    eval_failures.sort_by(|left, right| left.attr.cmp(&right.attr));
    Ok(EvalResult {
        checks,
        failures: eval_failures,
    })
}

/// The outcome of classifying one `nix-eval-jobs` run: the buildable checks, the
/// attrs that failed to evaluate (no derivation at this rev), and any rows of an
/// unexpected shape (neither drvPath nor error).
struct Partitioned {
    checks: Vec<Check>,
    eval_failures: Vec<EvalFailure>,
    unexpected: Vec<String>,
}

/// Parse one nix-eval-jobs JSONL stream and sort each row into [`Partitioned`].
/// Pure (no subprocess) so the success / eval-failure / malformed split is unit
/// tested without invoking nix.
fn partition_eval_rows(stdout: &str) -> Result<Partitioned> {
    let mut out = Partitioned {
        checks: Vec::new(),
        eval_failures: Vec::new(),
        unexpected: Vec::new(),
    };
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        let row: EvalRow =
            serde_json::from_str(line).with_context(|| format!("parse eval row: {line}"))?;
        // nix-eval-jobs quotes any attr-path segment that needs quoting in Nix
        // source (a dot inside the segment). A sharded `ciChecks` leaf can quote
        // an interior segment (`rust-foo."doctest-...lib.rs..."`), so unquote
        // each segment (see normalize_attr), not just the ends, before the bare
        // name flows through the diff, the report, and the workflow safename
        // regex.
        let attr = normalize_attr(&row.attr);
        match (row.drv_path, row.error) {
            (Some(drv_path), _) => out.checks.push(Check { attr, drv_path }),
            (None, Some(error)) => out.eval_failures.push(EvalFailure { attr, error }),
            (None, None) => out.unexpected.push(attr),
        }
    }
    Ok(out)
}

/// `nix derivation show` output: a `{ version, derivations }` envelope (schema 4+)
/// whose `derivations` map is keyed by `.drv` basename.
#[derive(Deserialize)]
struct ShowOutput {
    derivations: BTreeMap<String, ShowDrv>,
}

/// One derivation as `nix derivation show` reports it. Input derivations live
/// under `inputs.drvs` keyed by basename (older schemas used a top-level
/// `inputDrvs`; this targets the current schema the pinned Nix emits).
#[derive(Deserialize)]
struct ShowDrv {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    inputs: ShowInputs,
}

#[derive(Deserialize, Default)]
struct ShowInputs {
    #[serde(default)]
    drvs: BTreeMap<String, serde_json::Value>,
}

/// Load the recursive derivation graph rooted at `drv_paths`, keyed by `.drv`
/// basename. Used to walk down to the changed frontier.
pub fn derivation_graph(drv_paths: &[String]) -> Result<Graph> {
    if drv_paths.is_empty() {
        return Ok(Graph::new());
    }
    let mut args = vec![
        "derivation".to_owned(),
        "show".to_owned(),
        "--recursive".to_owned(),
        "--extra-experimental-features".to_owned(),
        "nix-command ca-derivations".to_owned(),
    ];
    args.extend(drv_paths.iter().cloned());
    let stdout = run(Command::new("nix").args(&args)).context("nix derivation show --recursive")?;

    let output: ShowOutput =
        serde_json::from_str(&stdout).context("parse nix derivation show output")?;
    Ok(output
        .derivations
        .into_iter()
        .map(|(name_key, drv)| {
            let name = drv.name.unwrap_or_else(|| drv_name(&name_key));
            let inputs = drv.inputs.drvs.into_keys().collect();
            (name_key, DrvNode { name, inputs })
        })
        .collect())
}

/// Reverse nix-eval-jobs' attr-path joining to a bare, schema-valid name.
///
/// nix-eval-jobs joins a nested attr path with `.`, wrapping any single segment
/// that contains a `.` in double quotes. The sharded `ciChecks` output nests
/// each crate's per-#[test] leaves under a `recurseForDerivations` group, so a
/// doctest case whose name carries a file path (`src/lib.rs - (line 12)`)
/// surfaces as `rust-foo."doctest-...src/lib.rs - (line 12)"`: the package
/// segment is bare, the leaf is quoted. nix-fast-build copies this joined string
/// verbatim into its `--timings` records (it ignores the `attrPath` array), so
/// the same shape reaches both [`eval_checks`] and [`crate::timings`].
///
/// The trusted workflow schema (`blast-radius.yml` safename regex) allows dots,
/// slashes, spaces, and parens but rejects `"`, and trimming only the ends
/// leaves the quotes around an interior segment in place. Drop every `"` and
/// split on dots outside quotes, then rejoin with `.`, so the bare path flows
/// identically through the diff key, the report, and the schema regardless of
/// which producer it came from. A flat single-segment name (quoted only because
/// its own case name holds a `.`) round-trips to the same bare name the old
/// end-trim produced.
pub fn normalize_attr(attr: &str) -> String {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in attr.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '.' if !in_quotes => segments.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    segments.push(current);
    segments.join(".")
}

/// Look up the derivation path for an attribute name in an evaluated set.
pub fn drv_for(checks: &[Check], attr: &str) -> Option<String> {
    checks
        .iter()
        .find(|check| check.attr == attr)
        .map(|check| check.drv_path.clone())
}

#[cfg(test)]
mod tests {
    use super::{EvalFailure, drv_name, normalize_attr, partition_eval_rows};

    #[test]
    fn drv_name_strips_hash_and_suffix() {
        assert_eq!(
            drv_name("/nix/store/abcdefghijklmnopqrstuvwxyz012345-ix-rust-workspace.drv"),
            "ix-rust-workspace"
        );
        // No hash prefix: left as-is (minus the suffix).
        assert_eq!(drv_name("plain-name.drv"), "plain-name");
        assert_eq!(drv_name("/nix/store/short.drv"), "short");
    }

    // A buildable row becomes a check; a per-attr eval failure is excluded (not a
    // rebuild target) rather than aborting the whole run; a malformed row with
    // neither field is flagged so the caller can fail loudly. Blank lines are
    // skipped. nix-eval-jobs quotes attr segments that need Nix quoting; the
    // quotes are stripped per segment, including a sharded leaf whose interior
    // segment is quoted.
    #[test]
    fn partition_splits_success_eval_failure_and_malformed() {
        let stdout = concat!(
            r#"{"attr":"rust-test-foo","drvPath":"/nix/store/aaa-foo.drv"}"#,
            "\n",
            // A sharded ciChecks doctest leaf: the package segment is bare, the
            // case segment is quoted because its name carries `lib.rs`.
            r#"{"attr":"rust-foo.\"doctest-src/lib.rs - (line 12)\"","drvPath":"/nix/store/bbb-doc.drv"}"#,
            "\n",
            r#"{"attr":"unfree-allowlist","error":"unfree allowlist mismatch"}"#,
            "\n",
            "\n",
            r#"{"attr":"\"weird.attr\""}"#,
            "\n",
        );

        let partitioned = partition_eval_rows(stdout).expect("well-formed JSONL parses");

        assert_eq!(partitioned.checks.len(), 2);
        assert_eq!(partitioned.checks[0].attr, "rust-test-foo");
        assert_eq!(partitioned.checks[0].drv_path, "/nix/store/aaa-foo.drv");
        // The quoted interior segment is unquoted but the dot path separator is
        // kept, so the bare name passes the workflow safename regex.
        assert_eq!(
            partitioned.checks[1].attr,
            "rust-foo.doctest-src/lib.rs - (line 12)"
        );
        assert_eq!(
            partitioned.eval_failures,
            vec![EvalFailure {
                attr: "unfree-allowlist".to_owned(),
                error: "unfree allowlist mismatch".to_owned(),
            }]
        );
        assert_eq!(partitioned.unexpected, vec!["weird.attr".to_owned()]);
    }

    // normalize_attr reverses nix-eval-jobs' attr-path join: drop quotes around
    // each segment, keep dots between segments.
    #[test]
    fn normalize_attr_unquotes_each_segment() {
        // Flat top-level name, no quoting.
        assert_eq!(normalize_attr("rust-test-foo"), "rust-test-foo");
        // Flat name quoted whole because its case carries a dot.
        assert_eq!(
            normalize_attr(r#""rust-foo-doctest-src/lib.rs - (line 12)""#),
            "rust-foo-doctest-src/lib.rs - (line 12)"
        );
        // Sharded path: bare package segment, quoted leaf segment.
        assert_eq!(
            normalize_attr(r#"rust-foo."doctest-src/lib.rs - (line 12)""#),
            "rust-foo.doctest-src/lib.rs - (line 12)"
        );
        // Sharded path with an unquoted leaf (no dot in the case name).
        assert_eq!(
            normalize_attr("rust-foo.causes-tests-some_case"),
            "rust-foo.causes-tests-some_case"
        );
    }
}
