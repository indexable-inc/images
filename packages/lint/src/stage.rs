//! The individual lint stages.
//!
//! Each stage's tools (alejandra, statix, deadnix, ruff, astlog, clone) come
//! from the wrapper PATH built in default.nix; discovery goes through
//! [`crate::walk`] for `fd` parity.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, ensure};
use regex::RegexSet;

use crate::walk;
use crate::walk::{FileQuery, Hidden};

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
// clone:ignore -- identifier-blind shape match with unibind's unrelated
// fieldless Ty enum (any two ~ten-variant unit enums collide).
pub enum Stage {
    Alejandra,
    Statix,
    Deadnix,
    Astlog,
    AstlogRust,
    AstlogElixir,
    Filenames,
    Dirnames,
    Ruff,
    Clone,
}

/// Run one stage, returning its exit code (tool failures and policy
/// violations are nonzero exit codes; only a broken invocation is an `Err`).
/// # Errors
/// When a stage's tool cannot be spawned or file discovery fails; a failing
/// lint is a nonzero return value, not an error.
pub fn run(stage: Stage) -> Result<i32> {
    match stage {
        Stage::Alejandra => alejandra(),
        Stage::Statix => run_tool("statix", &["check", "."]),
        // Strict: no `-L`/`--no-lambda-pattern-names`. That flag exists because
        // dropping a pattern name is unsafe without `...` in the pattern (it
        // narrows the callable signature); an unused name here must be deleted
        // (migrating call sites) or kept behind `...`, matching what the LSP
        // already flags as unused.
        Stage::Deadnix => run_tool("deadnix", &["--fail", "."]),
        Stage::Astlog => astlog(),
        Stage::AstlogRust => astlog_rust(),
        Stage::AstlogElixir => astlog_elixir(),
        Stage::Filenames => filenames(),
        Stage::Dirnames => dirnames(),
        Stage::Ruff => ruff(),
        Stage::Clone => clone_scan(),
    }
}

fn run_tool(program: &str, args: &[&str]) -> Result<i32> {
    run_command(Command::new(program).args(args))
}

fn run_command(command: &mut Command) -> Result<i32> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command
        .status()
        .with_context(|| format!("run {program}"))?;
    status
        .code()
        .with_context(|| format!("{program} was killed by a signal"))
}

fn nix_files() -> Result<Vec<String>> {
    walk::files(&FileQuery {
        extensions: &["nix"],
        hidden: Hidden::Skip,
        prune: &[],
    })
}

fn alejandra() -> Result<i32> {
    let files = nix_files()?;
    run_command(Command::new("alejandra").arg("--check").args(files))
}

/// The Nix style rules as astlog lint declarations
/// (astlog-rules/nix.astlog, #1060/#1062). `astlog scan` emits one
/// finding per lint-declared relation row and exits nonzero on any
/// error-severity finding, so adding a (lint ...) extends the gate
/// without touching this invocation. Legitimate exceptions are
/// suppressed in place with `astlog-ignore: <rule>` comments. Only
/// .nix files are handed to the corpus: astlog would otherwise parse
/// every known-grammar file in the repo to run nix-only rules.
fn astlog() -> Result<i32> {
    let files = nix_files()?;
    run_command(
        Command::new("astlog")
            .args(["scan", "astlog-rules/nix.astlog"])
            .args(files),
    )
}

/// The Rust style rules (astlog-rules/rust.astlog), the successor to the
/// ast-grep rust rules (#1060 ported the nix rules first). Scoped to the
/// corpus/search crates, the `files:` scope those rules carried under
/// ast-grep; astlog walks each directory and runs the rust rules over its
/// .rs files. Both rulesets share the `astlog-rules` flake-check self-test.
fn astlog_rust() -> Result<i32> {
    let dirs: Vec<&str> = [
        "packages/indexer",
        "packages/search",
        "packages/search-core",
        "packages/search-py",
        "packages/source",
        "packages/sink",
    ]
    .into_iter()
    .filter(|dir| Path::new(dir).exists())
    .collect();
    if !dirs.is_empty() {
        let code = run_command(
            Command::new("astlog")
                .args(["scan", "astlog-rules/rust.astlog"])
                .args(&dirs),
        )?;
        if code != 0 {
            return Ok(code);
        }
    }

    // The Cargo/workspace rules (astlog-rules/cargo.astlog, TOML grammar) run
    // over every Cargo.toml in the repo: `no-cargo-path-dep` bans inter-crate
    // `path` deps in member tables so local crates are declared once in a
    // [workspace.dependencies] and inherited with `workspace = true`. A
    // separate ruleset because the `astlog-rules` self-test maps one source
    // extension per ruleset (rust.astlog -> .rs, cargo.astlog -> .toml).
    let cargo_files = walk::files_named("Cargo.toml")?;
    if cargo_files.is_empty() {
        return Ok(0);
    }
    run_command(
        Command::new("astlog")
            .args(["scan", "astlog-rules/cargo.astlog"])
            .args(cargo_files),
    )
}

/// The Elixir lint rules (astlog-rules/elixir.astlog), two families. Type
/// discipline: a struct needs a `@type`, a public `def` needs a preceding
/// `@spec` (behaviour callbacks marked `@impl` are exempt), and a module
/// needs a `@moduledoc`: the lint-level nudge toward the shape Elixir
/// 1.18's set-theoretic checker can check. Correctness/security: no unsafe
/// dynamic atom creation (atom-table `DoS`), no leftover `IO.inspect`. Run
/// over every package's `lib/` Elixir, not a hand-maintained directory list:
/// the only scoping is to `lib/` itself, because `mix.exs` build functions
/// and `test/` `ExUnit` helpers are not the type-checked runtime surface and
/// speccing them would be noise. The walk already skips gitignored
/// `_build`/`deps`.
fn astlog_elixir() -> Result<i32> {
    let files: Vec<String> = walk::files(&FileQuery {
        extensions: &["ex", "exs"],
        hidden: Hidden::Skip,
        prune: &[],
    })?
    .into_iter()
    .filter(|path| path.starts_with("lib/") || path.contains("/lib/"))
    .collect();
    if files.is_empty() {
        return Ok(0);
    }
    run_command(
        Command::new("astlog")
            .args(["scan", "astlog-rules/elixir.astlog"])
            .args(files),
    )
}

/// Serialized-config filenames the repo allows; everything else with a
/// config-shaped extension must be a .nix expression instead.
const ALLOWED_FILENAMES: &[&str] = &[
    // Ecosystem-owned configuration and manifests.
    r"(^|/)Cargo\.toml$",
    r"(^|/)pyproject\.toml$",
    r"(^|/)rust-toolchain\.toml$",
    r"(^|/)mise\.toml$",
    r"(^|/)osv-scanner\.toml$",
    r"(^|/)ruff\.toml$",
    r"(^|/)statix\.toml$",
    r"(^|/)\.cargo/config\.toml$",
    r"^clone\.toml$",
    r"^packages/cve-scan/whitelist\.toml$",
    r"^\.github/.*\.ya?ml$",
    r"(^|/)docker-compose\.ya?ml$",
    r"(^|/)plugin\.yml$",
    r"^\.editorconfig$",
    r"^packages/minecraft/minestom/servers/[^/]+/gradle\.properties$",
    r"^packages/minecraft/minestom/servers/[^/]+/gradle/verification-metadata\.xml$",
    r"^packages/minecraft/minestom/servers/[^/]+/src/main/resources/logback\.xml$",
    // Gradle owns these root-build names; the catalog and verification
    // metadata are generated inputs shared by the Minestom subprojects.
    r"^packages/minecraft/minestom/gradle\.properties$",
    r"^packages/minecraft/minestom/gradle/libs\.versions\.toml$",
    r"^packages/minecraft/minestom/gradle/verification-metadata\.xml$",
    r"^packages/minecraft/minestom/gradle/snapshot-metadata\.xml$",
    // Generated manifests, locks, editor settings, and typed data.
    r"(^|/)(package|tsconfig)\.json$",
    r"(^|/)(package-lock|lock)\.json$",
    r"(^|/)(pins|manifest)\.json$",
    r"^\.(claude|vscode|zed)/settings\.json$",
    r"^\.vscode/extensions\.json$",
    r"^\.github/user-owners\.json$",
    r"(^|/)(dag|upstream-status)\.json$",
    r"(^|/)(fixtures?[^/]*|snapshots?|catalogs?|metadata|sounds|seeds)/.*\.json$",
    r"^examples/.*\.json$",
    r"^packages/agent/claude-code/system-prompts/models\.json$",
    r"^packages/agent/system-prompt-eval-viewer/src/sample\.json$",
    r"^packages/code/code-highlight/src/islands-theme\.json$",
    // Generated by `tree-sitter generate` and embedded by the grammar
    // crate's lib.rs (see packages/code/tree-sitter-nix/README.md).
    r"^packages/code/tree-sitter-nix/src/node-types\.json$",
    r"^tests/.*\.json$",
];

const CONFIG_EXTENSIONS: &[&str] = &[
    "toml",
    "json",
    "yaml",
    "yml",
    "kdl",
    "ini",
    "conf",
    "cfg",
    "xml",
    "properties",
    "editorconfig",
    "sobelow-conf",
];

fn denied_filenames(candidates: &[String]) -> Result<Vec<String>> {
    let allowed = RegexSet::new(ALLOWED_FILENAMES).context("compile filename allowlist")?;
    Ok(candidates
        .iter()
        .filter(|path| !allowed.is_match(path))
        .cloned()
        .collect())
}

/// Repository configuration belongs in composable Nix expressions. Keep
/// serialized files only where an external consumer owns the filename or
/// the file is generated data, a lock, a fixture, or a protocol payload.
fn filenames() -> Result<i32> {
    let candidates = walk::files(&FileQuery {
        extensions: CONFIG_EXTENSIONS,
        hidden: Hidden::Include,
        prune: &[".git", ".claude/worktrees"],
    })?;
    let denied = denied_filenames(&candidates)?;
    if denied.is_empty() {
        return Ok(0);
    }
    eprintln!(
        "prefer .nix for repository-owned configuration; serialized files require an external filename or generated/data role:"
    );
    for path in denied {
        eprintln!("  {path}");
    }
    Ok(1)
}

/// A grouping directory whose basename restates its parent's, with no
/// package root (package.nix / default.nix, the same markers
/// packages/registry.nix discovers by) anywhere on its prefix chain. An
/// eponym package inside its area (packages/nix/nix) is deliberate and
/// language layouts inside a package (the mcp server's Python
/// src/slack/slack) repeat a segment by convention; non-consecutive repeats
/// (foo/bar/foo) are fine and stay out of scope.
fn doubled_grouping_dirs(dirs: &[String], has_marker: impl Fn(&Path) -> bool) -> Vec<String> {
    dirs.iter()
        .filter(|dir| {
            let path = Path::new(dir.as_str());
            let base = path.file_name();
            let parent_base = path.parent().and_then(Path::file_name);
            if base.is_none() || base != parent_base {
                return false;
            }
            !path
                .ancestors()
                .filter(|scope| !scope.as_os_str().is_empty())
                .any(&has_marker)
        })
        .cloned()
        .collect()
}

/// The directory-tree form of the scopedNaming rule. The one occurrence,
/// packages/minecraft/minecraft/{bot,nbt,...}, was flattened into
/// packages/minecraft (b32885d); this stage keeps the doubled segment from
/// coming back.
fn dirnames() -> Result<i32> {
    let dirs = walk::directories_under("packages")?;
    let offenders = doubled_grouping_dirs(&dirs, |scope| {
        scope.join("package.nix").exists() || scope.join("default.nix").exists()
    });
    if offenders.is_empty() {
        return Ok(0);
    }
    eprintln!("grouping directory restates its parent's name; flatten the child into its parent:");
    for dir in offenders {
        eprintln!("  {dir}");
    }
    Ok(1)
}

/// Repo-wide Python lint: the shared ruff selector (bug-catchers + security +
/// pathlib + pytest + explicit annotations + no `typing.cast`; see
/// lib/ruff-ann.nix) over EVERY tracked .py, so non-package dirs
/// (tools/, users/, skills/, sdk/, examples/, lib/) are covered too, not just
/// the per-package build gates. The walk skips gitignored paths; `.claude`
/// (agent worktrees and assets) is filtered out explicitly. The selector argv
/// arrives as JSON in `IX_RUFF_ARGV` from the wrapper, so no shell fragment
/// is re-parsed here.
fn ruff() -> Result<i32> {
    let files: Vec<String> = walk::files(&FileQuery {
        extensions: &["py"],
        hidden: Hidden::Skip,
        prune: &[],
    })?
    .into_iter()
    .filter(|path| !path.starts_with(".claude/"))
    .collect();
    if files.is_empty() {
        return Ok(0);
    }
    let argv_json = std::env::var("IX_RUFF_ARGV")
        .context("IX_RUFF_ARGV is not set; run lint-stage via its wrapped package")?;
    let argv: Vec<String> =
        serde_json::from_str(&argv_json).context("parse IX_RUFF_ARGV as a JSON argv list")?;
    run_command(Command::new("ruff").arg("check").args(argv).args(files))
}

/// Code clone detection over the whole tree (packages/code/clone-detect).
/// `clone .` walks up for the repo `clone.toml`, whose `[budget]
/// global_pct` is the ceiling on whole-scan `duplication_pct`; the binary
/// exits nonzero when the global gate fails, so this gate ratchets
/// duplication down without failing on every pre-existing clone. Only the
/// global gate runs here: the diff gate needs a `.git` directory, and the
/// CI lint derivation copies a `.git`-less source tree. `clone` prints the
/// `DetectionResult` JSON to stdout; redirect it to null so a failing stage's
/// log shows the tracing gate summary (stderr), not the full JSON blob.
fn clone_scan() -> Result<i32> {
    let status = Command::new("clone")
        .arg(".")
        .stdout(Stdio::null())
        .status()
        .context("run clone")?;
    status.code().context("clone was killed by a signal")
}

/// Sanity net for the `RegexSet`: every allowlist pattern must compile on its
/// own, so a bad edit fails here rather than at first stage run.
/// # Errors
/// When any pattern fails to compile.
pub fn validate_allowlist() -> Result<()> {
    let compiled = RegexSet::new(ALLOWED_FILENAMES).context("compile filename allowlist")?;
    ensure!(
        compiled.len() == ALLOWED_FILENAMES.len(),
        "filename allowlist compiled to a different pattern count"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_compiles() {
        validate_allowlist().expect("allowlist compiles");
    }

    #[test]
    fn filename_policy_examples() {
        let candidates = vec![
            "packages/foo/Cargo.toml".to_owned(),
            ".editorconfig".to_owned(),
            ".github/workflows/check.yml".to_owned(),
            "repository-config.json".to_owned(),
            "zellij-layout.kdl".to_owned(),
            "packages/foo/fixtures/case.json".to_owned(),
        ];
        let denied = denied_filenames(&candidates).expect("allowlist compiles");
        assert_eq!(
            denied,
            vec!["repository-config.json".to_owned(), "zellij-layout.kdl".to_owned()]
        );
    }

    #[test]
    fn doubled_dirs_flag_markerless_and_exempt_package_roots() {
        let dirs = vec![
            "packages/foo/foo".to_owned(),
            "packages/bar/bar".to_owned(),
            "packages/baz/qux".to_owned(),
            "packages/a/b/a".to_owned(),
        ];
        let offenders = doubled_grouping_dirs(&dirs, |scope| {
            scope == Path::new("packages/bar/bar")
        });
        assert_eq!(offenders, vec!["packages/foo/foo".to_owned()]);
    }

    #[test]
    fn elixir_scope_matches_lib_prefix_only() {
        // Mirrors the `(^|/)lib/` scoping in astlog_elixir.
        let keep = |path: &str| path.starts_with("lib/") || path.contains("/lib/");
        assert!(keep("lib/app.ex"));
        assert!(keep("packages/hive/lib/hive.ex"));
        assert!(!keep("packages/hive/test/hive_test.exs"));
        assert!(!keep("toolib/x.ex"));
    }
}
