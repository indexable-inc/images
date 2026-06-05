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
use serde::{Deserialize, Serialize};

use crate::causes::{DrvNode, Graph};

/// Pinned `nix-eval-jobs` so evaluation behavior does not drift with the user's
/// channels. Matches the revision the old nushell tool used.
const EVAL_JOBS: &str =
    "github:nix-community/nix-eval-jobs/65ebf5b7cd453a27af09cf02b1fc57b3568cc4b7";

/// One evaluated check: its attribute name and the derivation it builds.
/// `Serialize`/`Deserialize` so a base eval can be cached on disk by SHA (see
/// `cache`); `PartialEq`/`Eq` back the cache round-trip test.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub attr: String,
    pub drv_path: String,
}

/// An attr that failed to evaluate, with the nix-eval-jobs error. Carried (not
/// just the name) so a fail-closed bail on a head regression can print WHY it
/// failed: per-attr eval failures exit nix-eval-jobs 0, so the error text is
/// otherwise never surfaced in CI logs.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalFailure {
    pub attr: String,
    pub error: String,
}

/// The result of evaluating `.#checks.x86_64-linux` at one rev: the buildable
/// checks, plus the attrs that failed to evaluate there (no derivation, so not a
/// rebuild target). The caller diffs `failures` across base and head to tell a
/// pre-existing catalog failure (tolerated) from one this change introduced.
#[derive(Debug, PartialEq, Eq)]
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

/// Evaluate every `.#checks.x86_64-linux` derivation at `rev` of the local repo.
///
/// `nix-eval-jobs` sits at the head of the pipeline; a startup/lock/fetch
/// failure surfaces here rather than yielding an empty set that silently
/// under-reports the blast radius.
pub fn eval_checks(repo: &str, rev: &str) -> Result<EvalResult> {
    let flakeref = format!("git+file://{repo}?rev={rev}&allRefs=1#checks.x86_64-linux");
    let stdout = run(Command::new("nix").args([
        "run",
        EVAL_JOBS,
        "--",
        "--flake",
        &flakeref,
        "--workers",
        "8",
        "--option",
        "accept-flake-config",
        "true",
        // The base eval predates any `nixConfig` declaration, so enable the
        // content-addressed feature directly rather than via the flake config.
        "--option",
        "extra-experimental-features",
        "ca-derivations",
        // The eval cache is left ON. It is keyed by the locked flake fingerprint,
        // and the flakeref above pins an immutable `?rev=<sha>` whose evaluation
        // is deterministic, so a cache entry for that rev can never be "stale":
        // it is exactly what a fresh eval would produce. Disabling it only forced
        // a full from-cold re-eval of the whole catalog every run. The runner's
        // `$HOME/.cache/nix` persists across jobs, so reruns of the same rev (and
        // the per-attr re-instantiation in `derivation_graph_for_attrs`) hit it.
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
        // nix-eval-jobs quotes attr segments that need quoting in Nix source
        // (dots, leading digits); strip them so the bare attribute name flows
        // through the diff, the report, and the workflow's safename regex.
        let attr = row.attr.trim_matches('"').to_owned();
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

/// Run `nix derivation show --recursive` over the given installables (either
/// `.drv` store paths or flakeref attrs) and parse the graph keyed by `.drv`
/// basename. `flakes` and `accept-flake-config` are enabled so a flakeref
/// installable (the cache-hit path) evaluates; they are harmless for store-path
/// installables.
fn derivation_show(installables: &[String]) -> Result<Graph> {
    if installables.is_empty() {
        return Ok(Graph::new());
    }
    let mut args = vec![
        "derivation".to_owned(),
        "show".to_owned(),
        "--recursive".to_owned(),
        "--extra-experimental-features".to_owned(),
        "nix-command flakes ca-derivations".to_owned(),
        "--option".to_owned(),
        "accept-flake-config".to_owned(),
        "true".to_owned(),
    ];
    args.extend(installables.iter().cloned());
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

/// Load the recursive derivation graph rooted at `drv_paths` (store paths),
/// keyed by `.drv` basename. Used to walk down to the changed frontier when the
/// roots' `.drv`s are already in the store (a freshly evaluated rev).
pub fn derivation_graph(drv_paths: &[String]) -> Result<Graph> {
    derivation_show(drv_paths)
}

/// Like `derivation_graph`, but addresses checks by attribute at `rev` rather
/// than by `.drv` path. Used when the base eval was served from cache: the base
/// eval was skipped, so the changed checks' base `.drv`s are not guaranteed to be
/// in the store. `nix derivation show` on a flakeref attr re-instantiates just
/// those few attrs and prints their graph, so only the changed frontier is
/// re-evaluated, not the whole catalog.
pub fn derivation_graph_for_attrs(repo: &str, rev: &str, attrs: &[String]) -> Result<Graph> {
    let installables: Vec<String> = attrs
        .iter()
        .map(|attr| {
            format!(
                "git+file://{repo}?rev={rev}&allRefs=1#checks.x86_64-linux.{}",
                attr_fragment_segment(attr)
            )
        })
        .collect();
    derivation_show(&installables)
}

/// Render `attr` as a flake-fragment attr-path segment, quoting only when the
/// name is not a bare segment (starts with a digit, or holds a char outside
/// `[A-Za-z0-9_-]` such as a `.`). Bare segments are left unquoted on purpose: a
/// quoted fragment trips a flakeref-parsing regression in some post-2.30 nix
/// nightlies (NixOS/nix#13772), and real check names (`rust-test-*`,
/// `eval-nixos-*`, `image-*`) are always bare, so the quoted form is reserved for
/// the rare attr that genuinely needs it.
fn attr_fragment_segment(attr: &str) -> String {
    let bare = !attr.is_empty()
        && !attr.starts_with(|c: char| c.is_ascii_digit())
        && attr
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare {
        attr.to_owned()
    } else {
        format!("\"{attr}\"")
    }
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
    use super::{EvalFailure, attr_fragment_segment, drv_name, partition_eval_rows};

    // Real check names are bare (no quoting), so the cache-hit cause path never
    // emits a quoted flake fragment for them; only a name that truly needs it
    // (a dot, or a leading digit) is quoted.
    #[test]
    fn attr_fragment_quotes_only_when_needed() {
        assert_eq!(attr_fragment_segment("rust-test-foo"), "rust-test-foo");
        assert_eq!(
            attr_fragment_segment("eval-nixos-hil-compute-2"),
            "eval-nixos-hil-compute-2"
        );
        assert_eq!(attr_fragment_segment("image_bar"), "image_bar");
        assert_eq!(attr_fragment_segment("weird.attr"), "\"weird.attr\"");
        assert_eq!(attr_fragment_segment("3leading"), "\"3leading\"");
    }

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
    // skipped. nix-eval-jobs quotes attrs that need Nix quoting; the quotes are
    // stripped.
    #[test]
    fn partition_splits_success_eval_failure_and_malformed() {
        let stdout = concat!(
            r#"{"attr":"rust-test-foo","drvPath":"/nix/store/aaa-foo.drv"}"#,
            "\n",
            r#"{"attr":"unfree-allowlist","error":"unfree allowlist mismatch"}"#,
            "\n",
            "\n",
            r#"{"attr":"\"weird.attr\""}"#,
            "\n",
        );

        let partitioned = partition_eval_rows(stdout).expect("well-formed JSONL parses");

        assert_eq!(partitioned.checks.len(), 1);
        assert_eq!(partitioned.checks[0].attr, "rust-test-foo");
        assert_eq!(partitioned.checks[0].drv_path, "/nix/store/aaa-foo.drv");
        assert_eq!(
            partitioned.eval_failures,
            vec![EvalFailure {
                attr: "unfree-allowlist".to_owned(),
                error: "unfree allowlist mismatch".to_owned(),
            }]
        );
        assert_eq!(partitioned.unexpected, vec!["weird.attr".to_owned()]);
    }
}
